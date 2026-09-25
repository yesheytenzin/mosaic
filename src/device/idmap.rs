//! `android.os.IIdmap2`, hosted here rather than by the image's `idmap2d`.
//!
//! The image's daemon does not implement the interface its own framework declares: it answers
//! `UNKNOWN_TRANSACTION` to `nextFabricatedOverlayInfos`, and `OverlayManagerService` blocks on
//! it during `startCoreServices`. The methods below are the ones this system can answer
//! *truthfully* -- there are no fabricated overlays and no idmaps here, so the iterators are
//! empty and the removals have nothing to remove -- and the rest are refused, which is what a
//! service that does not implement a method should say.
//!
//! `createIdmap` and `getIdmapPath` are the compiler's work. The bundle carries `idmap2` for it,
//! and they are a later piece of this same service rather than a different one.

use crate::binder::parcel::Parcel;
use crate::binder::{Answer, BinderObject};
use anyhow::Result;

/// The name the framework looks up.
///
/// `idmap`, not the interface's name: `IdmapDaemon` resolves it with
/// `ServiceManager.getService("idmap")`, and hosting the descriptor instead leaves the
/// lookup unanswered -- the service exists under a name nobody asks for. The descriptor is
/// what the proxy reads *after* it has the object, and that is `DESCRIPTOR` below.
pub const NAME: &str = "idmap";

/// The interface's own token, which a proxy reads before it will use the object.
const DESCRIPTOR: &str = "android.os.IIdmap2";

/// `Status::EX_UNSUPPORTED_OPERATION`, the same AIDL exception code the health service uses.
const EX_UNSUPPORTED_OPERATION: i32 = -7;

/// `IIdmap2`'s methods, numbered from one in the order the interface declares them. The order
/// was read out of the framework's own `services.jar` rather than guessed.
pub mod code {
    pub const ACQUIRE_FABRICATED_OVERLAY_ITERATOR: u32 = 1;
    pub const CREATE_FABRICATED_OVERLAY: u32 = 2;
    pub const CREATE_IDMAP: u32 = 3;
    pub const DELETE_FABRICATED_OVERLAY: u32 = 4;
    pub const DUMP_IDMAP: u32 = 5;
    pub const GET_IDMAP_PATH: u32 = 6;
    pub const NEXT_FABRICATED_OVERLAY_INFOS: u32 = 7;
    pub const RELEASE_FABRICATED_OVERLAY_ITERATOR: u32 = 8;
    pub const REMOVE_IDMAP: u32 = 9;
    pub const VERIFY_IDMAP: u32 = 10;
}

/// The iterator `acquireFabricatedOverlayIterator` hands back.
///
/// It has one method, `next()`, and this system has nothing for it to return -- so it says so,
/// which is what an iterator over an empty set does.
struct EmptyIterator;

impl BinderObject for EmptyIterator {
    fn descriptor(&self) -> &str {
        "android.os.IIdmap2$FabricatedOverlayIterator"
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        match code {
            // `next()`: no more. A null where the next info would be.
            1 => {
                let mut reply = Parcel::new();
                reply.null_binder();
                Ok(reply.into_bytes().into())
            }
            other => {
                // The iterator has one method; anything else is not implemented.
                let mut reply = Parcel::new();
                reply.i32(EX_UNSUPPORTED_OPERATION);
                reply.string16(&format!(
                    "{} method {} is not implemented",
                    "android.os.IIdmap2$FabricatedOverlayIterator", other
                ));
                reply.i32(0);
                Ok(reply.into_bytes().into())
            }
        }
    }
}

pub struct Idmap;

impl Idmap {
    pub fn new() -> Self {
        Idmap
    }

    /// A method this service does not implement, answered the way a stub answers one: an
    /// unsupported-operation status and the interface's name, which is what generated code
    /// writes for `UNKNOWN_TRANSACTION`.
    fn refuse(&self, code: u32) -> Answer {
        let mut reply = Parcel::new();
        reply.i32(EX_UNSUPPORTED_OPERATION);
        reply.string16(&format!(
            "{} method {} is not implemented",
            DESCRIPTOR, code
        ));
        reply.i32(0);
        reply.into_bytes().into()
    }
}

impl Default for Idmap {
    fn default() -> Self {
        Self::new()
    }
}

impl BinderObject for Idmap {
    fn descriptor(&self) -> &str {
        DESCRIPTOR
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        match code {
            // An iterator over nothing, because there are no fabricated overlays here.
            code::ACQUIRE_FABRICATED_OVERLAY_ITERATOR => {
                let mut reply = Parcel::new();
                let offset = reply.handle_binder(0);
                Ok(Answer::from(reply.into_bytes()).handing(
                    offset,
                    crate::binder::Handed::fresh(Box::new(EmptyIterator)),
                ))
            }
            // The one the boot blocks on. An empty list is the truth.
            code::NEXT_FABRICATED_OVERLAY_INFOS => {
                let mut reply = Parcel::new();
                reply.i32(0); // the array's length
                Ok(reply.into_bytes().into())
            }
            // Nothing to remove, nothing to delete, nothing to release: these succeed and do
            // not pretend otherwise.
            code::DELETE_FABRICATED_OVERLAY
            | code::RELEASE_FABRICATED_OVERLAY_ITERATOR
            | code::REMOVE_IDMAP => {
                let reply = Parcel::new();
                Ok(reply.into_bytes().into())
            }
            other => Ok(self.refuse(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> Idmap {
        Idmap::new()
    }

    #[test]
    fn it_says_which_interface_it_is() {
        // A proxy asks this before it will use the object, and answering wrongly is a
        // reported UNKNOWN_TRANSACTION.
        assert_eq!(service().descriptor(), "android.os.IIdmap2");
    }

    #[test]
    fn the_overlay_iterator_is_empty_rather_than_absent() {
        let mut service = service();
        let answer = service
            .transact(code::ACQUIRE_FABRICATED_OVERLAY_ITERATOR, &[])
            .unwrap();
        // The status word, then the object word the broker fills in.
        assert_eq!(answer.objects.len(), 1, "an iterator object");
    }

    #[test]
    fn there_are_no_fabricated_overlays_to_list() {
        let mut service = service();
        let answer = service
            .transact(code::NEXT_FABRICATED_OVERLAY_INFOS, &[])
            .unwrap();
        // The array's length, and nothing in front of it: no status here.
        let count = i32::from_le_bytes(answer.data[..4].try_into().unwrap());
        assert_eq!(count, 0, "an empty array, which is the truth here");
    }

    #[test]
    fn removals_with_nothing_to_remove_succeed() {
        for code in [
            code::DELETE_FABRICATED_OVERLAY,
            code::RELEASE_FABRICATED_OVERLAY_ITERATOR,
            code::REMOVE_IDMAP,
        ] {
            let answer = service().transact(code, &[]).unwrap();
            assert!(answer.data.is_empty(), "code {} has nothing to say", code);
        }
    }

    #[test]
    fn the_compilers_methods_are_refused_by_name() {
        for code in [code::CREATE_IDMAP, code::GET_IDMAP_PATH, code::VERIFY_IDMAP] {
            let answer = service().transact(code, &[]).unwrap();
            // A refusal writes its own status, because it is a status rather than a value:
            // the first word is `EX_UNSUPPORTED_OPERATION`.
            let status = i32::from_le_bytes(answer.data[..4].try_into().unwrap());
            assert_eq!(status, EX_UNSUPPORTED_OPERATION, "code {} is refused", code);
            // And a message follows it, which is what generated code does for
            // UNKNOWN_TRANSACTION.
            assert!(answer.data.len() > 4, "with a reason");
        }
    }
}
