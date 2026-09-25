// SPDX-License-Identifier: GPL-3.0-or-later

//! The health HAL, in userspace: this machine's battery.
//!
//! `BatteryService` asks `android.hardware.health.IHealth/default` for the
//! battery's state, and refuses to start without it (`NoSuchElementException:
//! IHealth service instance default isn't available`). On a device that is a HAL
//! reading the kernel's power supply class; on a laptop it is the same kernel's
//! same class, read directly -- `/sys/class/power_supply/BAT1/capacity` is the
//! percentage Android shows, `status` is charging or discharging, and the rest of
//! the numbers are there too. So this is the display half's shape: a device service
//! whose answers are about *this* machine.
//!
//! What the host does not have, this says it does not have rather than inventing
//! it: no wireless or dock charger, no battery cycle count where the kernel exposes
//! none, and a temperature only where the kernel reports one.

use crate::binder::parcel::Parcel;
use crate::binder::{Answer, BinderObject, PendingCall};
use anyhow::Result;

/// The name the framework looks up: `<interface>/<instance>`.
pub const NAME: &str = "android.hardware.health.IHealth/default";

/// The callback interface, which is what the receiver's `enforceInterface`
/// checks the parcel against.
const CALLBACK: &str = "android.hardware.health.IHealthInfoCallback";

/// `android.hardware.health.IHealth`, in declaration order.
mod code {
    /// `IHealthInfoCallback.onHealthInfoChanged`, the first method of that
    /// interface, which is the code a callback is made with.
    pub const ON_HEALTH_INFO_CHANGED: u32 = 1;
    pub const REGISTER_CALLBACK: u32 = 1;
    pub const UNREGISTER_CALLBACK: u32 = 2;
    pub const UPDATE: u32 = 3;
    pub const GET_CHARGE_COUNTER_UAH: u32 = 4;
    pub const GET_CURRENT_NOW_MICROAMPS: u32 = 5;
    pub const GET_CURRENT_AVERAGE_MICROAMPS: u32 = 6;
    pub const GET_CAPACITY: u32 = 7;
    pub const GET_ENERGY_COUNTER_NWH: u32 = 8;
    pub const GET_CHARGE_STATUS: u32 = 9;
    pub const GET_STORAGE_INFO: u32 = 10;
    pub const GET_DISK_STATS: u32 = 11;
    pub const GET_HEALTH_INFO: u32 = 12;
}

/// `android.hardware.health.BatteryStatus`.
mod status {
    pub const UNKNOWN: i32 = 1;
    pub const CHARGING: i32 = 2;
    pub const DISCHARGING: i32 = 3;
    pub const NOT_CHARGING: i32 = 4;
    pub const FULL: i32 = 5;
}

/// `android.hardware.health.BatteryHealth`: the battery is doing its job unless the
/// kernel says otherwise, and this host's kernel does not report a health file.
const HEALTH_GOOD: i32 = 2;
/// `android.hardware.health.BatteryCapacityLevel`: a percentage is a level, not one
/// of the named ones, so this is `UNKNOWN` rather than a guess.
const CAPACITY_LEVEL_UNKNOWN: i32 = 0;

/// `Status::EX_UNSUPPORTED_OPERATION`, as an AIDL exception code.
const EX_UNSUPPORTED_OPERATION: i32 = -7;

/// The battery, as the kernel's power supply class has it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Battery {
    /// The name under `/sys/class/power_supply`, e.g. `BAT1`.
    pub name: String,
    pub present: bool,
    pub capacity: i32,
    pub status: i32,
    pub voltage_millivolts: i32,
    pub temperature_tenths_celsius: i32,
    pub current_microamps: i32,
    pub charge_counter_uah: i32,
    pub cycle_count: i32,
    pub technology: String,
    /// Whether the AC adapter reports itself online.
    pub ac_online: bool,
    /// Whether a USB supply reports itself online.
    pub usb_online: bool,
}

/// Read one integer from a power supply attribute, or `default` where the kernel
/// does not expose it. A missing file is the normal case for half of these: a
/// laptop's battery reports capacity and status and nothing else.
fn read_number(dir: &std::path::Path, name: &str, default: i32) -> i32 {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(default)
}

fn read_text(dir: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name))
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// The host's battery, or none.
///
/// The kernel's `type` file is what says whether a supply is a battery: a laptop
/// also has `AC` and `ucsi-source-psy-*` in the same directory, and taking the first
/// entry would have made this report a charger as the battery.
pub fn battery() -> Option<Battery> {
    let root = std::path::Path::new("/sys/class/power_supply");
    let entries = std::fs::read_dir(root).ok()?;
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    names.sort();
    for name in names {
        let dir = root.join(&name);
        if read_text(&dir, "type") != "Battery" {
            continue;
        }
        let status = match read_text(&dir, "status").as_str() {
            "Charging" => status::CHARGING,
            "Discharging" => status::DISCHARGING,
            "Not charging" => status::NOT_CHARGING,
            "Full" => status::FULL,
            _ => status::UNKNOWN,
        };
        return Some(Battery {
            name,
            present: true,
            capacity: read_number(&dir, "capacity", 0),
            status,
            voltage_millivolts: read_number(&dir, "voltage_now", 0) / 1000,
            temperature_tenths_celsius: read_number(&dir, "temp", 0),
            current_microamps: read_number(&dir, "current_now", 0),
            charge_counter_uah: read_number(&dir, "charge_counter", 0),
            cycle_count: read_number(&dir, "cycle_count", 0),
            technology: read_text(&dir, "technology"),
            ac_online: false,
            usb_online: false,
        });
    }
    None
}

/// Whether any supply of this type is online.
fn supply_online(kind: &str) -> bool {
    let root = std::path::Path::new("/sys/class/power_supply");
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.filter_map(|entry| entry.ok()).any(|entry| {
        let dir = entry.path();
        read_text(&dir, "type") == kind && read_number(&dir, "online", 0) != 0
    })
}

pub struct Health {
    battery: Option<Battery>,
    ac_online: bool,
    usb_online: bool,
}

impl Health {
    pub fn new() -> Self {
        let battery = battery();
        let ac_online = supply_online("Mains");
        let usb_online = supply_online("USB");
        match &battery {
            Some(battery) => log::info!(
                "health: {} at {}%, {:?}, ac {} usb {}",
                battery.name,
                battery.capacity,
                battery.status,
                ac_online,
                usb_online
            ),
            None => log::info!("health: no battery in /sys/class/power_supply"),
        }
        Self {
            battery,
            ac_online,
            usb_online,
        }
    }

    /// The battery, or an absent one. An absent battery's status is `UNKNOWN`,
    /// which is a value the enum has; a zeroed default would report a status the
    /// framework does not define.
    fn present(&self) -> Battery {
        self.battery.clone().unwrap_or(Battery {
            status: status::UNKNOWN,
            ..Battery::default()
        })
    }

    /// `android.hardware.health.HealthInfo`, field by field, in the order its AIDL
    /// declaration lists them.
    /// `HealthInfo`'s wire form, which is a *sized* parcelable: the generated Java
    /// writes a length word first and patches it in afterwards, and its reader
    /// refuses a body shorter than four bytes -- `BadParcelableException: Parcelable
    /// too small` -- which is what these fields read as with no prefix in front of
    /// them. The
    /// length covers itself. Both callers need it: `getHealthInfo`'s answer, and the
    /// `HealthInfo` a callback carries.
    fn health_info(&self, out: &mut Parcel) {
        let body = self.health_info_fields();
        out.i32(body.len() as i32 + 4);
        out.raw(&body);
    }

    fn health_info_fields(&self) -> Vec<u8> {
        let mut out = Parcel::new();
        let battery = self.present();
        out.boolean(self.ac_online);
        out.boolean(self.usb_online);
        out.boolean(false); // chargerWirelessOnline: no wireless charger here
        out.boolean(false); // chargerDockOnline
        out.i32(0); // maxChargingCurrentMicroamps: the kernel does not report it
        out.i32(0); // maxChargingVoltageMicrovolts
        out.i32(battery.status);
        out.i32(HEALTH_GOOD);
        out.boolean(battery.present);
        out.i32(battery.capacity);
        out.i32(battery.voltage_millivolts);
        out.i32(battery.temperature_tenths_celsius);
        out.i32(battery.current_microamps);
        out.i32(battery.cycle_count);
        out.i32(0); // batteryFullChargeUah: not in the power supply class
        out.i32(battery.charge_counter_uah);
        out.string16(&battery.technology);
        out.i32(battery.current_microamps); // average: the instantaneous reading
        out.i32(0); // diskStats: no disks
        out.i32(0); // storageInfos: no storage
        out.i32(CAPACITY_LEVEL_UNKNOWN);
        out.i64(-1); // batteryChargeTimeToFullNowSeconds: unsupported
        out.i32(0); // batteryFullChargeDesignCapacityUah
        out.into_bytes()
    }

    fn refuse(&self, code: u32) -> Answer {
        let mut reply = Parcel::new();
        reply.i32(EX_UNSUPPORTED_OPERATION);
        reply.string16(&format!("IHealth method {} is not implemented", code));
        reply.i32(0); // the remote stack trace header the reader expects
        reply.into_bytes().into()
    }
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

impl BinderObject for Health {
    fn descriptor(&self) -> &str {
        "android.hardware.health.IHealth"
    }

    /// The one code that needs its argument: a callback registration.
    ///
    /// The caller hands over an object and waits to be called with it. Answering
    /// without ever calling it leaves `BatteryService` waiting for callbacks it
    /// will never get -- which is what it does now, 175 seconds at a time. A real
    /// health HAL reports the current state as soon as it is registered rather than
    /// only on a change, so that is what this asks the broker to do: one call, with
    /// the same `HealthInfo` a direct read would return.
    fn transact_with(&mut self, code: u32, data: &[u8], arguments: &[u32]) -> Result<Answer> {
        if code != code::REGISTER_CALLBACK {
            return self.transact(code, data);
        }
        let mut info = Parcel::new();
        self.health_info(&mut info);
        let mut answer: Answer = Answer::from(Vec::new());
        // The callback is a *Java* call, so it goes in the shape a Java receiver
        // enforces: the header, this descriptor, then the `HealthInfo`. Sending
        // the `HealthInfo` alone is what `BatteryService` read as
        // "Binder invocation to an incorrect interface" -- and it then waited out
        // its sixty-second timeout for a callback it had been sent.
        // The argument is a *typed* parcelable, which is a presence word and then the
        // parcelable: `readTypedObject` reads the presence first, and the parcelable's
        // own length second. Sending the parcelable alone puts its length where the
        // presence belongs, and the receiver takes the first *field* for the length
        // and refuses it -- `BadParcelableException: Parcelable too small`, at
        // `HealthInfo.readFromParcel` under `Parcel.readTypedObject`.
        let mut args = Parcel::new();
        args.parcelable_present();
        args.raw(&info.into_bytes());
        let data = crate::binder::parcel::java_call(CALLBACK, &args.into_bytes());
        // No handle means the caller passed no object, which is a registration that
        // cannot be honoured rather than one to pretend about.
        if let Some(handle) = arguments.first() {
            answer.calls.push(PendingCall {
                handle: *handle,
                code: code::ON_HEALTH_INFO_CHANGED,
                data,
            });
        }
        Ok(answer)
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        match code {
            // What `BatteryService` asks for: the whole state in one call.
            code::GET_HEALTH_INFO => {
                let mut reply = Parcel::new();
                // The same presence word the callback carries: `getHealthInfo` returns a
                // typed parcelable, and the proxy reads the presence before the body.
                reply.parcelable_present();
                self.health_info(&mut reply);
                Ok(reply.into_bytes().into())
            }
            code::GET_CAPACITY => {
                let mut reply = Parcel::new();
                reply.i32(self.present().capacity);
                Ok(reply.into_bytes().into())
            }
            code::GET_CHARGE_STATUS => {
                let mut reply = Parcel::new();
                reply.i32(self.present().status);
                Ok(reply.into_bytes().into())
            }
            code::GET_CHARGE_COUNTER_UAH => {
                let mut reply = Parcel::new();
                reply.i32(self.present().charge_counter_uah);
                Ok(reply.into_bytes().into())
            }
            code::GET_CURRENT_NOW_MICROAMPS | code::GET_CURRENT_AVERAGE_MICROAMPS => {
                let mut reply = Parcel::new();
                reply.i32(self.present().current_microamps);
                Ok(reply.into_bytes().into())
            }
            code::GET_ENERGY_COUNTER_NWH => {
                let mut reply = Parcel::new();
                // The power supply class has charge in microamp-hours, not energy:
                // voltage times charge, which is what a reader of this wants.
                let battery = self.present();
                let micro_watt_hours = (battery.charge_counter_uah as i64)
                    * (battery.voltage_millivolts as i64)
                    / 1000;
                reply.i64(micro_watt_hours * 1000);
                Ok(reply.into_bytes().into())
            }
            // A callback registered and never called: nothing here watches the
            // battery for changes. The framework tolerates that -- it reads the
            // state when it needs it -- and it is the truth rather than a lie about
            // events that would not arrive.
            code::REGISTER_CALLBACK | code::UNREGISTER_CALLBACK | code::UPDATE => {
                let reply = Parcel::new();
                Ok(reply.into_bytes().into())
            }
            // No storage and no disks to report: an empty array is the shape, and
            // the count of zero says it.
            code::GET_STORAGE_INFO | code::GET_DISK_STATS => {
                let mut reply = Parcel::new();
                reply.i32(0);
                Ok(reply.into_bytes().into())
            }
            other => Ok(self.refuse(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::parcel::Reader;

    fn service() -> Health {
        Health {
            battery: Some(Battery {
                name: "BAT1".to_string(),
                present: true,
                capacity: 78,
                status: status::DISCHARGING,
                voltage_millivolts: 12000,
                temperature_tenths_celsius: 305,
                current_microamps: -900000,
                charge_counter_uah: 3500000,
                cycle_count: 120,
                technology: "Li-ion".to_string(),
                ac_online: false,
                usb_online: false,
            }),
            ac_online: false,
            usb_online: true,
        }
    }

    #[test]
    fn the_health_info_has_the_fields_the_aidl_declares() {
        let mut service = service();
        let reply = service.transact(code::GET_HEALTH_INFO, &[]).unwrap().data;
        let mut reader = Reader::new(&reply);
        // The parcelable's length comes first and covers itself: a body that says it
        // is shorter than four bytes is refused by the Java reader
        // (`BadParcelableException: Parcelable too small`), which is how a
        // `HealthInfo` with no prefix reads.
        assert_eq!(reader.i32(), 1); // the parcelable is present
        assert_eq!(reader.i32() as usize, reply.len() - 4); // its length covers itself
        assert_eq!(reader.i32(), 0); // chargerAcOnline
        assert_eq!(reader.i32(), 1); // chargerUsbOnline
        assert_eq!(reader.i32(), 0); // chargerWirelessOnline
        assert_eq!(reader.i32(), 0); // chargerDockOnline
        assert_eq!(reader.i32(), 0); // maxChargingCurrentMicroamps
        assert_eq!(reader.i32(), 0); // maxChargingVoltageMicrovolts
        assert_eq!(reader.i32(), status::DISCHARGING);
        assert_eq!(reader.i32(), HEALTH_GOOD);
        assert_eq!(reader.i32(), 1); // batteryPresent
        assert_eq!(reader.i32(), 78); // batteryLevel
        assert_eq!(reader.i32(), 12000); // batteryVoltageMillivolts
        assert_eq!(reader.i32(), 305); // batteryTemperatureTenthsCelsius
        assert_eq!(reader.i32(), -900000); // batteryCurrentMicroamps
        assert_eq!(reader.i32(), 120); // batteryCycleCount
        assert_eq!(reader.i32(), 0); // batteryFullChargeUah
        assert_eq!(reader.i32(), 3500000); // batteryChargeCounterUah
        assert_eq!(reader.string().as_deref(), Some("Li-ion"));
        assert_eq!(reader.i32(), -900000); // batteryCurrentAverageMicroamps
        assert_eq!(reader.i32(), 0); // diskStats
        assert_eq!(reader.i32(), 0); // storageInfos
        assert_eq!(reader.i32(), CAPACITY_LEVEL_UNKNOWN);
        assert_eq!(reader.i64(), -1); // batteryChargeTimeToFullNowSeconds
        assert_eq!(reader.i32(), 0); // batteryFullChargeDesignCapacityUah
    }

    #[test]
    fn the_cheap_readers_answer_the_same_battery() {
        let mut service = service();
        for (code, expected) in [
            (code::GET_CAPACITY, 78),
            (code::GET_CHARGE_STATUS, status::DISCHARGING),
            (code::GET_CHARGE_COUNTER_UAH, 3500000),
            (code::GET_CURRENT_NOW_MICROAMPS, -900000),
        ] {
            let reply = service.transact(code, &[]).unwrap().data;
            let mut reader = Reader::new(&reply);
            assert_eq!(reader.i32(), expected, "method {code}");
        }
    }

    #[test]
    fn a_machine_with_no_battery_says_so_rather_than_guessing() {
        let mut service = Health {
            battery: None,
            ac_online: true,
            usb_online: false,
        };
        let reply = service.transact(code::GET_HEALTH_INFO, &[]).unwrap().data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), 1); // present
        assert_eq!(reader.i32() as usize, reply.len() - 4); // its own length
        assert_eq!(reader.i32(), 1); // ac online
        assert_eq!(reader.i32(), 0); // usb offline
        assert_eq!(reader.i32(), 0); // wireless
        assert_eq!(reader.i32(), 0); // dock
        assert_eq!(reader.i32(), 0); // maxChargingCurrentMicroamps
        assert_eq!(reader.i32(), 0); // maxChargingVoltageMicrovolts
        assert_eq!(reader.i32(), status::UNKNOWN); // no status to report
        assert_eq!(reader.i32(), HEALTH_GOOD);
        assert_eq!(reader.i32(), 0); // not present
    }

    #[test]
    fn the_rest_is_refused_by_name() {
        let mut service = service();
        let reply = service.transact(99, &[]).unwrap().data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), EX_UNSUPPORTED_OPERATION);
        assert!(reader
            .string()
            .unwrap_or_default()
            .contains("IHealth method 99"));
    }
}
