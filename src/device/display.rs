// SPDX-License-Identifier: GPL-3.0-or-later

//! The display manager HAL, in userspace: this machine's displays.
//!
//! `DisplayManagerService` looks up **`display`**, not `SurfaceFlinger`, and what is behind that
//! name is `android.hardware.display.IDisplayManager`. Hosting the SurfaceFlinger HAL there was
//! wrong, and the way it was wrong is worth keeping: the display manager's first call, out of its
//! own constructor, is **code 40**, `getPreferredWideGamutColorSpaceId`. The SurfaceFlinger HAL's
//! catch-all refusal answered `Status::EX_UNSUPPORTED_OPERATION` -- `-7` -- and the framework fed
//! that straight into `ColorSpace.get(-7)`, which is an array index:
//!
//! ```text
//! java.lang.ArrayIndexOutOfBoundsException: length=16; index=-7
//!   at android.app.ResourcesManager.getDisplayMetrics(ResourcesManager.java:325)
//! ```
//!
//! Sixteen is the number of named color spaces. A refusal where the caller expected a value is
//! worse than no service at all, which is the lesson `idmap` taught first.
//!
//! The transaction codes below are `IDisplayManager.aidl`'s **declaration order**, which is what
//! AIDL numbers transactions by. The dex lists methods alphabetically, so reading the codes out of
//! it is wrong -- it agreed with itself and disagreed with the wire.
//!
//! The displays themselves are the host's, read the way `surfaceflinger` reads them -- DRM's sysfs.
//! What this machine does not have, this says it does not have: no wifi display, no virtual
//! display compositor, and brightness belongs to the desktop, so the calls that would set it are
//! accepted rather than refused.

use crate::binder::parcel::{args_after, Parcel};
use crate::binder::{Answer, BinderObject};
use crate::device::surfaceflinger::{self, Display};
use anyhow::Result;

/// The name the framework looks up.
pub const NAME: &str = "display";

/// The interface behind it.
pub const AIDL: &str = "android.hardware.display.IDisplayManager";

/// `android.hardware.display.IDisplayManager`, in `IDisplayManager.aidl`'s declaration order.
mod code {
    pub const GET_DISPLAY_INFO: u32 = 1;
    pub const GET_DISPLAY_IDS: u32 = 2;
    pub const IS_UID_PRESENT_ON_DISPLAY: u32 = 3;
    pub const REGISTER_CALLBACK: u32 = 4;
    pub const REGISTER_CALLBACK_WITH_EVENT_MASK: u32 = 5;
    pub const START_WIFI_DISPLAY_SCAN: u32 = 6;
    pub const STOP_WIFI_DISPLAY_SCAN: u32 = 7;
    pub const CONNECT_WIFI_DISPLAY: u32 = 8;
    pub const DISCONNECT_WIFI_DISPLAY: u32 = 9;
    pub const RENAME_WIFI_DISPLAY: u32 = 10;
    pub const FORGET_WIFI_DISPLAY: u32 = 11;
    pub const PAUSE_WIFI_DISPLAY: u32 = 12;
    pub const RESUME_WIFI_DISPLAY: u32 = 13;
    pub const GET_WIFI_DISPLAY_STATUS: u32 = 14;
    pub const SET_USER_DISABLED_HDR_TYPES: u32 = 15;
    pub const SET_ARE_USER_DISABLED_HDR_TYPES_ALLOWED: u32 = 16;
    pub const ARE_USER_DISABLED_HDR_TYPES_ALLOWED: u32 = 17;
    pub const GET_USER_DISABLED_HDR_TYPES: u32 = 18;
    pub const REQUEST_COLOR_MODE: u32 = 19;
    pub const CREATE_VIRTUAL_DISPLAY: u32 = 20;
    pub const RESIZE_VIRTUAL_DISPLAY: u32 = 21;
    pub const SET_VIRTUAL_DISPLAY_SURFACE: u32 = 22;
    pub const RELEASE_VIRTUAL_DISPLAY: u32 = 23;
    pub const SET_VIRTUAL_DISPLAY_STATE: u32 = 24;
    /// The call the boot died on.
    pub const GET_STABLE_DISPLAY_SIZE: u32 = 25;
    pub const GET_BRIGHTNESS_EVENTS: u32 = 26;
    pub const GET_AMBIENT_BRIGHTNESS_STATS: u32 = 27;
    pub const SET_BRIGHTNESS_CONFIGURATION_FOR_USER: u32 = 28;
    pub const SET_BRIGHTNESS_CONFIGURATION_FOR_DISPLAY: u32 = 29;
    pub const GET_BRIGHTNESS_CONFIGURATION_FOR_DISPLAY: u32 = 30;
    pub const GET_BRIGHTNESS_CONFIGURATION_FOR_USER: u32 = 31;
    pub const GET_DEFAULT_BRIGHTNESS_CONFIGURATION: u32 = 32;
    pub const IS_MINIMAL_POST_PROCESSING_REQUESTED: u32 = 33;
    pub const SET_TEMPORARY_BRIGHTNESS: u32 = 34;
    pub const SET_BRIGHTNESS: u32 = 35;
    pub const GET_BRIGHTNESS: u32 = 36;
    pub const SET_TEMPORARY_AUTO_BRIGHTNESS_ADJUSTMENT: u32 = 37;
    pub const GET_MINIMUM_BRIGHTNESS_CURVE: u32 = 38;
    pub const GET_BRIGHTNESS_INFO: u32 = 39;
    /// The call the boot died on. `ColorSpaces.get(-7)` is where the refusal surfaced.
    pub const GET_PREFERRED_WIDE_GAMUT_COLOR_SPACE_ID: u32 = 40;
    pub const SET_USER_PREFERRED_DISPLAY_MODE: u32 = 41;
    pub const GET_USER_PREFERRED_DISPLAY_MODE: u32 = 42;
    pub const GET_SYSTEM_PREFERRED_DISPLAY_MODE: u32 = 43;
    pub const SET_SHOULD_ALWAYS_RESPECT_APP_REQUESTED_MODE: u32 = 44;
    pub const SHOULD_ALWAYS_RESPECT_APP_REQUESTED_MODE: u32 = 45;
    pub const SET_REFRESH_RATE_SWITCHING_TYPE: u32 = 46;
    pub const GET_REFRESH_RATE_SWITCHING_TYPE: u32 = 47;
    pub const GET_DISPLAY_DECORATION_SUPPORT: u32 = 48;
}

/// `ColorSpace.Named.SRGB`'s id. The wide-gamut slot's value *means* "no wide gamut support" when
/// it is sRGB, which is the truth for a host compositor handing an app a surface in whatever space
/// it was given -- and it is a value that exists, which is the part that matters: a refusal here
/// is `-7`, and `-7` is an exception raised inside the framework's own color-space table.
const COLOR_SPACE_SRGB: i32 = 0;

/// `Display.TYPE_EXTERNAL`: a desktop monitor is not an internal panel.
const TYPE_EXTERNAL: i32 = 2;
/// `Display.STATE_ON`.
const STATE_ON: i32 = 2;
/// `Display.REMOVE_MODE_MOVE_CONTENT_TO_PRIMARY`.
const REMOVE_MODE_MOVE_CONTENT_TO_PRIMARY: i32 = 0;
/// `Display.DEFAULT_DISPLAY`. The logical id the default display has always had, and the id
/// `DisplayManagerService` hands out for the built-in panel.
const DEFAULT_DISPLAY: i32 = 0;
/// `Process.SYSTEM_UID`: the display belongs to the system, which is who owns the panel.
const SYSTEM_UID: i32 = 1000;
/// `Display.PREFERRED_REFRESH_RATE` is the panel's; this is the density the framework itself uses
/// for a display whose physical size it cannot read (mdpi).
const DEFAULT_DENSITY_DPI: i32 = 160;

/// `Parcel::writeString8`: a byte length, then the bytes and their terminator, padded out to four.
fn string8(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    out.extend_from_slice(&(bytes.len() as i32).to_le_bytes());
    out.extend_from_slice(bytes);
    out.push(0);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

/// `Parcel::writeParcelable` of a null: a null class name, which is `writeString`'s `-1`.
fn null_parcelable(reply: &mut Parcel) {
    reply.i32(-1);
}

/// One display's `android.view.DisplayInfo`, exactly as the framework's `DisplayInfo.writeToParcel`
/// writes it. The order is not a guess: it is that method's, read from the AOSP source for the
/// build this bundle is (SDK 33), and the bundle's `classes3.dex` carries every one of these
/// fields.
///
/// A field written in the wrong place is not a wrong value, it is a different *type* in that slot,
/// which is how a `Rect[]` length came to be read as a `DisplayCutout`'s bounds:
/// `bad array lengths` in `readTypedArray`.
fn display_info(display: &Display, logical_id: i32) -> Vec<u8> {
    let mut out = Vec::new();
    let (width, height) = (display.width as i32, display.height as i32);
    out.extend_from_slice(&0i32.to_le_bytes()); // layerStack
    out.extend_from_slice(&0i32.to_le_bytes()); // flags
    out.extend_from_slice(&TYPE_EXTERNAL.to_le_bytes());
    out.extend_from_slice(&logical_id.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // displayGroupId
                                                // address and deviceProductInfo: a null class name each. Both are nullable, and inventing a
                                                // product descriptor would be a claim about the panel no one here can make.
    out.extend_from_slice(&(-1i32).to_le_bytes());
    out.extend_from_slice(&(-1i32).to_le_bytes());
    string8(&mut out, &display.connector); // name
    out.extend_from_slice(&width.to_le_bytes()); // appWidth
    out.extend_from_slice(&height.to_le_bytes()); // appHeight
    out.extend_from_slice(&width.to_le_bytes()); // smallestNominalAppWidth
    out.extend_from_slice(&height.to_le_bytes()); // smallestNominalAppHeight
    out.extend_from_slice(&width.to_le_bytes()); // largestNominalAppWidth
    out.extend_from_slice(&height.to_le_bytes()); // largestNominalAppHeight
    out.extend_from_slice(&width.to_le_bytes()); // logicalWidth
    out.extend_from_slice(&height.to_le_bytes()); // logicalHeight
                                                  // `DisplayCutout.ParcelableWrapper.writeCutoutToParcel`: -1 is null, 0 is NO_CUTOUT, 1 then the
                                                  // payload is a real cutout. A desktop monitor has none, and NO_CUTOUT is the answer that says
                                                  // so without claiming the display is gone.
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // rotation: the compositor's, not ours
    out.extend_from_slice(&0i32.to_le_bytes()); // modeId
    out.extend_from_slice(&0i32.to_le_bytes()); // defaultModeId
    out.extend_from_slice(&1i32.to_le_bytes()); // supportedModes.length
                                                // Display.Mode.writeToParcel: id, width, height, refresh rate, alternative rates.
    out.extend_from_slice(&0i32.to_le_bytes()); // the mode's id
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&display.refresh_hz.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // no alternative refresh rates
    out.extend_from_slice(&0i32.to_le_bytes()); // colorMode: COLOR_MODE_NATIVE
    out.extend_from_slice(&1i32.to_le_bytes()); // supportedColorModes.length
    out.extend_from_slice(&0i32.to_le_bytes()); // COLOR_MODE_NATIVE
    out.extend_from_slice(&(-1i32).to_le_bytes()); // hdrCapabilities: null
    out.extend_from_slice(&0i32.to_le_bytes()); // minimalPostProcessingSupported
    out.extend_from_slice(&DEFAULT_DENSITY_DPI.to_le_bytes());
    out.extend_from_slice(&160f32.to_le_bytes()); // physicalXDpi
    out.extend_from_slice(&160f32.to_le_bytes()); // physicalYDpi
    out.extend_from_slice(&0i64.to_le_bytes()); // appVsyncOffsetNanos
    out.extend_from_slice(&0i64.to_le_bytes()); // presentationDeadlineNanos
    out.extend_from_slice(&STATE_ON.to_le_bytes());
    out.extend_from_slice(&SYSTEM_UID.to_le_bytes()); // ownerUid
    out.extend_from_slice(&(-1i32).to_le_bytes()); // ownerPackageName: null
    string8(&mut out, &format!("mosaic:{}", display.connector)); // uniqueId
    out.extend_from_slice(&REMOVE_MODE_MOVE_CONTENT_TO_PRIMARY.to_le_bytes());
    out.extend_from_slice(&0f32.to_le_bytes()); // refreshRateOverride: none
    out.extend_from_slice(&0f32.to_le_bytes()); // brightnessMinimum
    out.extend_from_slice(&1f32.to_le_bytes()); // brightnessMaximum
    out.extend_from_slice(&0.5f32.to_le_bytes()); // brightnessDefault
    out.extend_from_slice(&0i32.to_le_bytes()); // roundedCorners: null
    out.extend_from_slice(&0i32.to_le_bytes()); // userDisabledHdrTypes.length
    out.extend_from_slice(&0i32.to_le_bytes()); // installOrientation: ROTATION_0
    out
}

/// The display manager, served over the socket the way the other device services are.
pub struct DisplayManager {
    displays: Vec<Display>,
}

impl DisplayManager {
    /// The host's displays. Called by the broker at startup.
    pub fn reporting() -> Self {
        Self {
            displays: surfaceflinger::displays(),
        }
    }

    /// The display the args name. `getDisplayInfo` and friends take a *logical* id, which is this
    /// index -- the default display has been id 0 for the life of the platform.
    fn named(&self, args: &[u8]) -> Option<&Display> {
        let id = args
            .get(..4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(DEFAULT_DISPLAY);
        self.displays.get(id.max(0) as usize)
    }

    fn answer(&mut self, code: u32, args: &[u8]) -> Answer {
        match code {
            // `DisplayInfo getDisplayInfo(int)`: an AIDL parcelable return, so a presence word
            // first and then the fields.
            code::GET_DISPLAY_INFO => {
                let mut reply = Parcel::new();
                let id = args
                    .get(..4)
                    .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
                    .unwrap_or(DEFAULT_DISPLAY);
                match self.named(args) {
                    Some(display) => {
                        reply.i32(1);
                        reply.raw(&display_info(display, id));
                    }
                    None => reply.i32(0), // a null DisplayInfo
                }
                reply.into_bytes().into()
            }
            // `int[] getDisplayIds(boolean)`: the logical ids, which are 0 up.
            code::GET_DISPLAY_IDS => {
                let mut reply = Parcel::new();
                reply.i32(self.displays.len() as i32);
                for index in 0..self.displays.len() {
                    reply.i32(index as i32);
                }
                reply.into_bytes().into()
            }
            // `Point getStableDisplaySize()`: the size that does not change, which is the mode's.
            code::GET_STABLE_DISPLAY_SIZE => {
                let mut reply = Parcel::new();
                let first = self.displays.first();
                reply.i32(1); // the Point is present
                reply.i32(first.map(|d| d.width as i32).unwrap_or(0));
                reply.i32(first.map(|d| d.height as i32).unwrap_or(0));
                reply.into_bytes().into()
            }
            // `int getPreferredWideGamutColorSpaceId()`. sRGB is a *valid* id and means "no wide
            // gamut" -- the same answer `SurfaceFlinger` gives for `GET_COMPOSITION_PREFERENCE`.
            // The boot died here: a refusal is -7, and the framework indexes its color space table
            // with that.
            code::GET_PREFERRED_WIDE_GAMUT_COLOR_SPACE_ID => {
                let mut reply = Parcel::new();
                reply.i32(COLOR_SPACE_SRGB);
                reply.into_bytes().into()
            }
            // `BrightnessInfo getBrightnessInfo(int)`, in its own field order.
            code::GET_BRIGHTNESS_INFO => {
                let mut reply = Parcel::new();
                reply.i32(1); // present
                reply.raw(&0.5f32.to_le_bytes()); // brightness
                reply.raw(&f32::NAN.to_le_bytes()); // adjustedBrightness: none applied
                reply.raw(&0f32.to_le_bytes()); // brightnessMinimum
                reply.raw(&1f32.to_le_bytes()); // brightnessMaximum
                reply.i32(0); // highBrightnessMode: none
                reply.raw(&f32::NAN.to_le_bytes()); // highBrightnessTransitionPoint
                reply.i32(0); // brightnessMaxReason: none
                reply.into_bytes().into()
            }
            // `float getBrightness(int)`.
            code::GET_BRIGHTNESS => {
                let mut reply = Parcel::new();
                reply.raw(&0.5f32.to_le_bytes());
                reply.into_bytes().into()
            }
            // `boolean isUidPresentOnDisplay(int, int)`: every uid is on the one display there is.
            code::IS_UID_PRESENT_ON_DISPLAY => {
                let mut reply = Parcel::new();
                reply.boolean(true);
                reply.into_bytes().into()
            }
            // With no HDR panel support claimed anywhere, no HDR type is disabled and none is
            // being withheld from the user.
            code::ARE_USER_DISABLED_HDR_TYPES_ALLOWED => {
                let mut reply = Parcel::new();
                reply.boolean(true);
                reply.into_bytes().into()
            }
            code::GET_USER_DISABLED_HDR_TYPES => {
                let mut reply = Parcel::new();
                reply.empty_array();
                reply.into_bytes().into()
            }
            // `int getRefreshRateSwitchingType()`: `NONE`. This side does not switch the host's
            // refresh rate; the compositor does.
            code::GET_REFRESH_RATE_SWITCHING_TYPE => {
                let mut reply = Parcel::new();
                reply.i32(0);
                reply.into_bytes().into()
            }
            // Nothing is asked of this side about app-requested modes, and nothing here acts on
            // them.
            code::SHOULD_ALWAYS_RESPECT_APP_REQUESTED_MODE
            | code::IS_MINIMAL_POST_PROCESSING_REQUESTED => {
                let mut reply = Parcel::new();
                reply.boolean(false);
                reply.into_bytes().into()
            }
            // Brightness belongs to the desktop. The framework asks this side to set it, and the
            // honest answer is to accept: there is nothing here to set, and refusing leaves an
            // exception where the caller expected a value.
            code::SET_BRIGHTNESS
            | code::SET_TEMPORARY_BRIGHTNESS
            | code::SET_TEMPORARY_AUTO_BRIGHTNESS_ADJUSTMENT
            | code::SET_USER_DISABLED_HDR_TYPES
            | code::SET_ARE_USER_DISABLED_HDR_TYPES_ALLOWED
            | code::SET_BRIGHTNESS_CONFIGURATION_FOR_USER
            | code::SET_BRIGHTNESS_CONFIGURATION_FOR_DISPLAY
            | code::SET_SHOULD_ALWAYS_RESPECT_APP_REQUESTED_MODE
            | code::SET_USER_PREFERRED_DISPLAY_MODE
            | code::SET_REFRESH_RATE_SWITCHING_TYPE
            | code::REQUEST_COLOR_MODE
            | code::REGISTER_CALLBACK
            | code::REGISTER_CALLBACK_WITH_EVENT_MASK => Answer::from(Vec::new()),
            // A wifi display is a second, wireless display and there is no such service on a Linux
            // host: the scan calls and the status are answered with nothing to report.
            code::START_WIFI_DISPLAY_SCAN
            | code::STOP_WIFI_DISPLAY_SCAN
            | code::CONNECT_WIFI_DISPLAY
            | code::DISCONNECT_WIFI_DISPLAY
            | code::RENAME_WIFI_DISPLAY
            | code::FORGET_WIFI_DISPLAY
            | code::PAUSE_WIFI_DISPLAY
            | code::RESUME_WIFI_DISPLAY => Answer::from(Vec::new()),
            code::GET_WIFI_DISPLAY_STATUS => {
                let mut reply = Parcel::new();
                reply.i32(0); // no status: there is no wireless display service
                reply.into_bytes().into()
            }
            // The brightness history and configuration are records of choices a user made in an
            // Android settings screen. There is no such history here, and null is how the framework
            // says it has none.
            code::GET_AMBIENT_BRIGHTNESS_STATS
            | code::GET_BRIGHTNESS_EVENTS
            | code::GET_BRIGHTNESS_CONFIGURATION_FOR_DISPLAY
            | code::GET_BRIGHTNESS_CONFIGURATION_FOR_USER
            | code::GET_DEFAULT_BRIGHTNESS_CONFIGURATION
            | code::GET_MINIMUM_BRIGHTNESS_CURVE
            | code::GET_USER_PREFERRED_DISPLAY_MODE
            | code::GET_SYSTEM_PREFERRED_DISPLAY_MODE
            | code::GET_DISPLAY_DECORATION_SUPPORT => {
                let mut reply = Parcel::new();
                null_parcelable(&mut reply);
                reply.into_bytes().into()
            }
            // A virtual display is the compositing half -- a surface, a consumer, a scheduler --
            // and none of that is here. This is where a refusal is honest and harmless: the
            // framework asks for one and handles the failure.
            code::CREATE_VIRTUAL_DISPLAY
            | code::RESIZE_VIRTUAL_DISPLAY
            | code::SET_VIRTUAL_DISPLAY_SURFACE
            | code::RELEASE_VIRTUAL_DISPLAY
            | code::SET_VIRTUAL_DISPLAY_STATE => self.refuse(code),
            other => self.refuse(other),
        }
    }

    /// `Status::EX_UNSUPPORTED_OPERATION` with the message the reader expects, for the calls that
    /// are genuinely not this side's to answer.
    fn refuse(&self, code: u32) -> Answer {
        let mut reply = Parcel::new();
        reply.i32(-7); // EX_UNSUPPORTED_OPERATION
        reply.string16(&format!("{AIDL} method {code} is not implemented"));
        reply.i32(0); // the remote stack trace header the reader expects
        reply.into_bytes().into()
    }
}

impl BinderObject for DisplayManager {
    fn transact(&mut self, code: u32, data: &[u8]) -> Result<Answer> {
        Ok(self.answer(code, args_after(data, AIDL)))
    }

    fn descriptor(&self) -> &str {
        AIDL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One display, built the way the SurfaceFlinger tests build theirs: the answers are about the
    /// host's displays, and a test that read `/sys/class/drm` would be a test of this machine
    /// rather than of this code.
    fn service() -> DisplayManager {
        DisplayManager {
            displays: vec![Display {
                id: 1 << 32,
                width: 1920,
                height: 1080,
                refresh_hz: 60.0,
                connector: "card1-eDP-1".to_string(),
            }],
        }
    }

    #[test]
    fn the_display_ids_are_logical_and_the_info_describes_them() {
        let mut service = service();
        let ids = service.answer(code::GET_DISPLAY_IDS, &[]).data;
        let mut reader = crate::binder::parcel::Reader::new(&ids);
        assert_eq!(reader.i32s(), vec![DEFAULT_DISPLAY]); // the count, then the ids

        let mut args = DEFAULT_DISPLAY.to_le_bytes().to_vec();
        args.extend_from_slice(&0u32.to_le_bytes()); // includeDisabled
        let info = service.answer(code::GET_DISPLAY_INFO, &args).data;
        let mut reader = crate::binder::parcel::Reader::new(&info);
        assert_eq!(reader.i32(), 1, "the DisplayInfo is present");
        assert_eq!(reader.i32(), 0); // layerStack
        assert_eq!(reader.i32(), 0); // flags
        assert_eq!(reader.i32(), TYPE_EXTERNAL);
        assert_eq!(reader.i32(), DEFAULT_DISPLAY);
        assert_eq!(reader.i32(), 0); // displayGroupId
        assert_eq!(reader.i32(), -1); // address: null
        assert_eq!(reader.i32(), -1); // deviceProductInfo: null
        assert_eq!(reader.string8().as_deref(), Some("card1-eDP-1"));
        assert_eq!(reader.i32(), 1920); // appWidth
        assert_eq!(reader.i32(), 1080); // appHeight
    }

    /// The size the framework asks for out of its constructor, and the color space id that used to
    /// be a refusal. `-7` there is what `ColorSpace.get(-7)` indexed its table with.
    #[test]
    fn the_stable_size_and_the_color_space_id_are_values() {
        let mut service = service();
        let size = service.answer(code::GET_STABLE_DISPLAY_SIZE, &[]).data;
        let mut reader = crate::binder::parcel::Reader::new(&size);
        assert_eq!(reader.i32(), 1); // the Point is present
        assert_eq!(reader.i32(), 1920, "the Point's x");
        assert_eq!(reader.i32(), 1080);

        let space = service
            .answer(code::GET_PREFERRED_WIDE_GAMUT_COLOR_SPACE_ID, &[])
            .data;
        let mut reader = crate::binder::parcel::Reader::new(&space);
        assert_eq!(reader.i32(), COLOR_SPACE_SRGB);
    }

    /// Brightness is the desktop's, and the calls that would set it must be accepted rather than
    /// refused -- a refusal is an exception where the caller expected a value.
    #[test]
    fn setting_brightness_is_accepted_not_refused() {
        let mut service = service();
        for code in [
            code::SET_TEMPORARY_AUTO_BRIGHTNESS_ADJUSTMENT,
            code::SET_TEMPORARY_BRIGHTNESS,
            code::SET_BRIGHTNESS,
        ] {
            let answer = service.answer(code, &[]).data;
            assert!(
                answer.is_empty(),
                "code {code} answered {} bytes",
                answer.len()
            );
        }
    }

    /// A virtual display is the compositing half, which is not here, and saying so is a real
    /// exception rather than a value.
    #[test]
    fn a_virtual_display_is_refused() {
        let mut service = service();
        let answer = service.answer(code::CREATE_VIRTUAL_DISPLAY, &[]).data;
        let mut reader = crate::binder::parcel::Reader::new(&answer);
        assert_eq!(reader.i32(), -7);
    }
}
