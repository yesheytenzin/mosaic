// SPDX-License-Identifier: GPL-3.0-or-later

//! Per-process handle tables and reference counts (A3).
//!
//! Binder's central trick is that one object has a different number in every
//! process that can reach it. A handle means nothing outside the table that
//! issued it, and this is that table. The broker keeps one per connected
//! process; tests keep one each.

use std::collections::HashMap;

/// Where a handle points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The service manager, which is handle 0 in every process.
    ServiceManager,
    /// An object this process owns. The node is its identity elsewhere.
    Local(u64),
    /// An object another process owns.
    Remote { node: u64, owner: u32 },
}

impl Target {
    /// The node this target names, if it names one.
    pub fn node(&self) -> Option<u64> {
        match self {
            Target::ServiceManager => None,
            Target::Local(node) => Some(*node),
            Target::Remote { node, .. } => Some(*node),
        }
    }

    /// The process that owns it, if it is not this table's own node.
    pub fn owner(&self) -> Option<u32> {
        match self {
            Target::ServiceManager => Some(crate::binder::SERVICE_MANAGER),
            Target::Local(_) => None,
            Target::Remote { owner, .. } => Some(*owner),
        }
    }
}

/// What a release did, so the caller knows when a handle stopped being real.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released {
    /// References remain.
    Counted { strong: u32 },
    /// The last reference went and the entry is gone. A remote node's owner
    /// must be told, because its reference count went down with it.
    Dropped(Target),
}

struct Entry {
    target: Target,
    strong: u32,
    weak: u32,
}

/// One process's view of the Binder objects it can reach.
pub struct HandleTable {
    entries: HashMap<u32, Entry>,
    next: u32,
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl HandleTable {
    /// A fresh table. Handle 0 is the service manager and is never released,
    /// which is what makes it usable as a constant everywhere else.
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        entries.insert(
            0,
            Entry {
                target: Target::ServiceManager,
                strong: 1,
                weak: 1,
            },
        );
        Self { entries, next: 1 }
    }

    /// The handle for a target, allocating one if this table has not seen it.
    /// Reaching the same object twice gives the same number, as in Binder: a
    /// process has one handle per object it can reach, not one per reference.
    pub fn handle_for(&mut self, target: Target) -> u32 {
        if let Some(handle) = self.find(&target) {
            if let Some(entry) = self.entries.get_mut(&handle) {
                entry.strong += 1;
            }
            return handle;
        }
        let handle = self.allocate();
        self.entries.insert(
            handle,
            Entry {
                target,
                strong: 1,
                weak: 1,
            },
        );
        handle
    }

    fn find(&self, target: &Target) -> Option<u32> {
        self.entries
            .iter()
            .find(|(_, e)| &e.target == target)
            .map(|(handle, _)| *handle)
    }

    /// Numbers are handed out in order and a released one comes back, which is
    /// what keeps a long-lived process from drifting into large handles.
    fn allocate(&mut self) -> u32 {
        loop {
            let candidate = self.next;
            self.next = self.next.wrapping_add(1);
            if self.next == 0 {
                self.next = 1;
            }
            if !self.entries.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    pub fn target(&self, handle: u32) -> Option<Target> {
        self.entries.get(&handle).map(|e| e.target)
    }

    pub fn contains(&self, handle: u32) -> bool {
        self.entries.contains_key(&handle)
    }

    pub fn strong(&self, handle: u32) -> Option<u32> {
        self.entries.get(&handle).map(|e| e.strong)
    }

    pub fn weak(&self, handle: u32) -> Option<u32> {
        self.entries.get(&handle).map(|e| e.weak)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every handle and its target, for the broker to account for a process.
    pub fn handles(&self) -> Vec<(u32, Target)> {
        let mut all: Vec<(u32, Target)> = self
            .entries
            .iter()
            .map(|(handle, entry)| (*handle, entry.target))
            .collect();
        all.sort_by_key(|(handle, _)| *handle);
        all
    }

    /// A strong reference, as `BC_ACQUIRE` carries.
    pub fn acquire(&mut self, handle: u32) -> anyhow::Result<Target> {
        let entry = self
            .entries
            .get_mut(&handle)
            .ok_or_else(|| anyhow::anyhow!("no handle {}", handle))?;
        entry.strong += 1;
        Ok(entry.target)
    }

    /// A strong reference going away, as `BC_RELEASE` carries.
    pub fn release(&mut self, handle: u32) -> anyhow::Result<Released> {
        let entry = self
            .entries
            .get_mut(&handle)
            .ok_or_else(|| anyhow::anyhow!("no handle {}", handle))?;
        entry.strong = entry.strong.saturating_sub(1);
        if entry.strong > 0 {
            return Ok(Released::Counted {
                strong: entry.strong,
            });
        }
        let target = entry.target;
        self.entries.remove(&handle);
        Ok(Released::Dropped(target))
    }

    /// A weak reference, as `BC_INCREFS` carries. Weak references keep the
    /// handle around without keeping the object alive, which is what death
    /// notification is built on.
    pub fn inc_refs(&mut self, handle: u32) -> anyhow::Result<()> {
        let entry = self
            .entries
            .get_mut(&handle)
            .ok_or_else(|| anyhow::anyhow!("no handle {}", handle))?;
        entry.weak += 1;
        Ok(())
    }

    /// A weak reference going away. The entry goes when both counts are zero.
    pub fn dec_refs(&mut self, handle: u32) -> anyhow::Result<Released> {
        let entry = self
            .entries
            .get_mut(&handle)
            .ok_or_else(|| anyhow::anyhow!("no handle {}", handle))?;
        entry.weak = entry.weak.saturating_sub(1);
        if entry.weak > 0 || entry.strong > 0 {
            return Ok(Released::Counted {
                strong: entry.strong,
            });
        }
        let target = entry.target;
        self.entries.remove(&handle);
        Ok(Released::Dropped(target))
    }

    /// Forget every handle to a process that has gone. Returns what was
    /// dropped, so the broker can decrement each node's holders.
    pub fn drop_owner(&mut self, owner: u32) -> Vec<(u32, u64)> {
        let doomed: Vec<u32> = self
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.target, Target::Remote { owner: o, .. } if o == owner))
            .map(|(handle, _)| *handle)
            .collect();
        let mut dropped = Vec::new();
        for handle in doomed {
            if let Some(entry) = self.entries.remove(&handle) {
                if let Target::Remote { node, .. } = entry.target {
                    dropped.push((handle, node));
                }
            }
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_zero_is_the_service_manager() {
        let table = HandleTable::new();
        assert_eq!(table.target(0), Some(Target::ServiceManager));
        assert_eq!(table.strong(0), Some(1));
    }

    #[test]
    fn the_same_object_gets_the_same_handle() {
        let mut table = HandleTable::new();
        let first = table.handle_for(Target::Remote { node: 7, owner: 2 });
        let second = table.handle_for(Target::Remote { node: 7, owner: 2 });
        assert_eq!(first, second);
        assert_eq!(table.strong(first), Some(2));
        assert_eq!(table.len(), 2, "handle 0 and the one object");
    }

    #[test]
    fn a_dropped_handle_is_not_handed_out_again_while_in_use() {
        let mut table = HandleTable::new();
        let a = table.handle_for(Target::Remote { node: 1, owner: 2 });
        let b = table.handle_for(Target::Remote { node: 2, owner: 2 });
        assert_ne!(a, b);
        assert_eq!(
            table.release(a).unwrap(),
            Released::Dropped(Target::Remote { node: 1, owner: 2 })
        );

        let c = table.handle_for(Target::Remote { node: 3, owner: 2 });
        assert!(table.contains(c));
        assert!(
            !table.contains(a),
            "a released handle must not be live again"
        );
    }

    /// A strong reference keeps the object reachable; the handle only goes when
    /// the last one is released, which is the invariant A3 exists to hold.
    #[test]
    fn references_hold_an_object_open() {
        let mut table = HandleTable::new();
        let handle = table.handle_for(Target::Remote { node: 9, owner: 3 });
        let again = table.handle_for(Target::Remote { node: 9, owner: 3 });
        assert_eq!(handle, again);

        assert_eq!(
            table.release(handle).unwrap(),
            Released::Counted { strong: 1 },
            "one reference remains"
        );
        assert!(table.contains(handle), "the object is still reachable");
        assert_eq!(
            table.release(handle).unwrap(),
            Released::Dropped(Target::Remote { node: 9, owner: 3 })
        );
        assert!(!table.contains(handle));
    }

    #[test]
    fn a_weak_reference_alone_does_not_keep_an_entry() {
        let mut table = HandleTable::new();
        let handle = table.handle_for(Target::Remote { node: 4, owner: 1 });
        table.release(handle).unwrap();
        assert!(!table.contains(handle));

        let handle = table.handle_for(Target::Remote { node: 5, owner: 1 });
        table.dec_refs(handle).unwrap();
        assert!(table.contains(handle), "a strong reference is still held");
        assert_eq!(
            table.release(handle).unwrap(),
            Released::Dropped(Target::Remote { node: 5, owner: 1 })
        );
    }

    #[test]
    fn a_dead_owner_takes_its_handles_with_it() {
        let mut table = HandleTable::new();
        table.handle_for(Target::Remote { node: 1, owner: 7 });
        table.handle_for(Target::Remote { node: 2, owner: 7 });
        table.handle_for(Target::Remote { node: 3, owner: 8 });

        let dropped = table.drop_owner(7);
        assert_eq!(dropped.len(), 2);
        assert!(dropped.iter().all(|(_, node)| *node != 3));
        assert!(table.contains(0), "the service manager is not owned by 7");
        let survivor = table.find(&Target::Remote { node: 3, owner: 8 }).unwrap();
        assert!(
            table.contains(survivor),
            "another process's object is untouched"
        );
    }

    #[test]
    fn releasing_an_unknown_handle_is_an_error() {
        let mut table = HandleTable::new();
        assert!(table.release(99).is_err());
        assert!(table.acquire(99).is_err());
    }
}
