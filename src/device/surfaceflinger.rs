// SPDX-License-Identifier: GPL-3.0-or-later

//! `SurfaceFlinger`, in userspace: the display half.
//!
//! On a device this is the compositor: it owns the displays, the layer tree and the
//! GPU, and every window an app draws into is one of its surfaces. Here it is the
//! part the system server waits for before it will start anything else --
//! `DisplayManagerService` wants to know what displays exist, what modes they have
//! and what state they are in, and it waits for the name to appear before it asks.
//!
//! The compositing half -- surfaces, `BLASTBufferQueue`, dma-buf, EGL -- is the
//! rest of the windowing phase (ADR-0006): an app's window on this host is a
//! Wayland surface, which is a subsystem rather than a service, and nothing here
//! pretends to have one.
//!
//! # Two interfaces, one name
//!
//! Android 13 has *two* `ISurfaceComposer` interfaces and both answer to the name
//! `SurfaceFlinger`:
//!
//! | Interface | Descriptor | Codes |
//! | --- | --- | --- |
//! | `android::gui::ISurfaceComposer` (AIDL) | `android.gui.ISurfaceComposer` | its declaration order, 1..21 |
//! | `android::ISurfaceComposer` (older) | `android.ui.ISurfaceComposer` | the tags in `ISurfaceComposer.h` |
//!
//! They overlap -- `bootFinished` is code 1 in the older one and `createDisplay` is
//! code 1 in the AIDL one -- and the framework uses both: `SurfaceComposerClient`
//! reaches the AIDL methods for everything the AIDL file declares, and the older
//! tags for what it does not (`getCompositionPreference`, `getStaticDisplayInfo`,
//! `getDynamicDisplayInfo`). Which one a request is for is decided by the interface
//! token the request carries, which is the same thing `CHECK_INTERFACE` reads on a
//! device.
//!
//! The codes and layouts here are not from memory: the older ones are the tags in
//! `ISurfaceComposer.h` and the write sequences in `ISurfaceComposer.cpp` of the
//! branch this runtime bundle is built from (Android 13, `TQ3A.230901.001`), and
//! the AIDL ones were read out of the bundle's own `libgui.so` -- the immediate
//! passed to `transact` in `BpSurfaceComposer::getPhysicalDisplayIds` is 3, which
//! is that method's place in the AIDL declaration order.

use crate::binder::parcel::{args_after, carries_token, Parcel};
use crate::binder::{Answer, BinderObject, Handed};
use anyhow::Result;

/// The name the framework looks up for the older interface.
pub const NAME: &str = "SurfaceFlinger";
/// And the one it looks up for the AIDL interface. Android 13 registers both, under
/// two names rather than one: the same object cannot report two interface
/// descriptors, and each client checks the descriptor it expects.
pub const AIDL_NAME: &str = "SurfaceFlingerAIDL";

/// The AIDL interface, which is what this reports itself as.
pub const AIDL: &str = "android.gui.ISurfaceComposer";
/// The older interface, whose tags are a different numbering.
pub const LEGACY: &str = "android.ui.ISurfaceComposer";

/// `Status::EX_UNSUPPORTED_OPERATION`, as an AIDL exception code.
const EX_UNSUPPORTED_OPERATION: i32 = -7;

/// `android.gui.ISurfaceComposer`, in declaration order. Confirmed against the
/// bundle's own `libgui.so`: `getPhysicalDisplayIds` is 3 in both.
mod aidl {
    pub const CREATE_DISPLAY: u32 = 1;
    pub const DESTROY_DISPLAY: u32 = 2;
    pub const GET_PHYSICAL_DISPLAY_IDS: u32 = 3;
    pub const GET_PRIMARY_PHYSICAL_DISPLAY_ID: u32 = 4;
    pub const GET_PHYSICAL_DISPLAY_TOKEN: u32 = 5;
    pub const SET_POWER_MODE: u32 = 6;
    pub const GET_DISPLAY_STATS: u32 = 7;
    pub const GET_DISPLAY_STATE: u32 = 8;
    pub const CLEAR_BOOT_DISPLAY_MODE: u32 = 9;
    pub const GET_BOOT_DISPLAY_MODE_SUPPORT: u32 = 10;
    pub const SET_AUTO_LOW_LATENCY_MODE: u32 = 11;
    pub const SET_GAME_CONTENT_TYPE: u32 = 12;
    pub const IS_WIDE_COLOR_DISPLAY: u32 = 16;
    pub const GET_DISPLAY_BRIGHTNESS_SUPPORT: u32 = 17;
    pub const SET_DISPLAY_BRIGHTNESS: u32 = 18;
    pub const NOTIFY_POWER_BOOST: u32 = 21;
}

/// `android::ISurfaceComposer`, in the order of `ISurfaceComposer.h`'s tag enum.
///
/// The numbers are the position of each name in that enum, counted from
/// `IBinder::FIRST_CALL_TRANSACTION`. They are not the ones an eye lands on: the
/// list has seventy-one entries and reading down it by hand missed five, which is
/// how `getDynamicDisplayInfo` came to be sent as 52 when the framework sends 55.
/// The framework's own request settled it -- and then counting the enum
/// programmatically agreed.
mod legacy {
    pub const BOOT_FINISHED: u32 = 1;
    pub const CREATE_CONNECTION: u32 = 2;
    pub const GET_STATIC_DISPLAY_INFO: u32 = 3;
    pub const CREATE_DISPLAY_EVENT_CONNECTION: u32 = 4;
    pub const GET_COMPOSITION_PREFERENCE: u32 = 27;
    pub const GET_PROTECTED_CONTENT_SUPPORT: u32 = 32;
    pub const GET_DISPLAY_NATIVE_PRIMARIES: u32 = 34;
    pub const GET_DESIRED_DISPLAY_MODE_SPECS: u32 = 39;
    pub const GET_GPU_CONTEXT_PRIORITY: u32 = 53;
    pub const GET_MAX_ACQUIRED_BUFFER_COUNT: u32 = 54;
    pub const GET_DYNAMIC_DISPLAY_INFO: u32 = 55;
    pub const GET_DISPLAY_DECORATION_SUPPORT: u32 = 67;
}

/// `ui::ColorMode::NATIVE`: the display's own mode, which is the only one a host
/// output has. Anything else would be a claim about color management that nothing
/// here does.
const COLOR_MODE_NATIVE: i32 = 0;
/// `ui::Dataspace::V0_SRGB`. For the wide-gamut slot this value *means* "no wide
/// color gamut support" (`ISurfaceComposer.aidl`), which is the truth for a host
/// compositor that hands the app a surface in whatever space it was given.
const DATASPACE_SRGB: i32 = 142671872;
/// `ui::PixelFormat::RGBA_8888`.
const PIXEL_FORMAT_RGBA_8888: i32 = 1;
/// `ui::DisplayConnectionType::External`: a desktop monitor is not an internal
/// panel, and saying so is more useful than defaulting to `Internal`.
const CONNECTION_TYPE_EXTERNAL: i32 = 1;
/// `ui::DisplayState::ON`.
const DISPLAY_STATE_ON: i32 = 2;
/// `ui::Rotation::Rotation0`.
const ROTATION_0: i32 = 0;
/// The refresh rate reported when the host does not say. DRM's sysfs exposes the
/// modes as `widthxheight` and nothing else -- no timing, no refresh -- and the
/// compositor that does know is a Wayland client this side is not. 60 is the
/// framework's own baseline for a display that reports nothing.
const DEFAULT_REFRESH_HZ: f32 = 60.0;
/// The density reported when the panel's physical size cannot be read. Android's
/// baseline (mdpi); a Wayland output would give the millimetres.
const DEFAULT_DENSITY_DPI: f32 = 160.0;

/// One display, as this host has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// Android's `PhysicalDisplayId`, opaque to everything above: the port in the
    /// high half, so it is stable across calls and never zero.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f32,
    /// The connector it came from, for the log and for the token's identity.
    pub connector: String,
}

impl Display {
    fn density_dpi(&self) -> f32 {
        DEFAULT_DENSITY_DPI
    }
}

/// The host's displays, from DRM's sysfs.
///
/// A connector that is `connected` and has modes is a display; the first mode is
/// the preferred one, which is what a compositor starts in. This is the host's own
/// view of its hardware rather than a Wayland client's -- what the *compositor* is
/// doing with it (rotation, scaling, which output is primary) is the desktop's
/// business and not visible from here.
pub fn displays() -> Vec<Display> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return found;
    };
    let mut connectors: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.contains('-') && name.starts_with("card"))
                .unwrap_or(false)
        })
        .collect();
    connectors.sort();
    for (index, path) in connectors.iter().enumerate() {
        let status = std::fs::read_to_string(path.join("status")).unwrap_or_default();
        if status.trim() != "connected" {
            continue;
        }
        let modes = std::fs::read_to_string(path.join("modes")).unwrap_or_default();
        let Some((width, height)) = modes.lines().find_map(mode_size) else {
            continue;
        };
        let connector = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("display")
            .to_string();
        found.push(Display {
            // The port in the high half, so the id is stable, opaque and never
            // collides with the null id.
            id: ((index as u64) + 1) << 32,
            width,
            height,
            refresh_hz: DEFAULT_REFRESH_HZ,
            connector,
        });
    }
    found
}

/// A mode line from DRM: `1920x1080`.
fn mode_size(line: &str) -> Option<(u32, u32)> {
    let (width, height) = line.trim().split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// The object a caller gets for a display and passes back on later calls.
///
/// On a device this is a binder that lives in SurfaceFlinger's layer hierarchy;
/// here it is only an identity, which is all the framework uses it for: it keeps
/// the token from `getPhysicalDisplayToken` and hands it back to ask about that
/// display. The node travels with it so two calls for the same display give the
/// same object rather than two that mean the same thing.
pub struct DisplayToken {
    pub id: u64,
}

impl BinderObject for DisplayToken {
    fn descriptor(&self) -> &str {
        "android.gui.IDisplayToken"
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        let mut reply = Parcel::new();
        reply.i32(EX_UNSUPPORTED_OPERATION);
        reply.string16(&format!("a display token answers nothing (code {})", code));
        reply.i32(0);
        Ok(reply.into_bytes().into())
    }
}
/// The identity returned for a surface created before the compositor exists.
/// It deliberately has no drawing or transaction implementation.
pub struct SurfaceHandle;
/// The connection returned by `createConnection`.
///
/// `SurfaceComposerClient` requires a non-null `ISurfaceComposerClient` before
/// it will construct a `SurfaceControl`. Returning null made `nativeCreate` fail
/// with `NO_INIT`; this is the smallest object that lets the framework finish
/// constructing its display surfaces. It carries an identity, not a compositor:
/// the object below refuses the operations that would require a real layer tree.
pub struct SurfaceComposerConnection;

impl BinderObject for SurfaceComposerConnection {
    fn descriptor(&self) -> &str {
        "android.ui.ISurfaceComposerClient"
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        match code {
            // `createSurfaceChecked` writes the returned surface binder, the
            1 => {
                let mut reply = Parcel::new();
                let handle = reply.handle_binder(0);
                let producer = reply.handle_binder(0);
                reply.i32(1); // generated layer id
                reply.i32(0); // transform hint
                reply.i32(0); // status
                Ok(Answer::from(reply.into_bytes())
                    .handing(handle, Handed::fresh(Box::new(SurfaceHandle)))
                    .handing(producer, Handed::fresh(Box::new(BufferProducer))))
            }
            other => {
                let mut reply = Parcel::new();
                reply.i32(EX_UNSUPPORTED_OPERATION);
                reply.string16(&format!(
                    "ISurfaceComposerClient method {other} is not implemented"
                ));
                reply.i32(0);
                Ok(reply.into_bytes().into())
            }
        }
    }
}

impl BinderObject for SurfaceHandle {
    fn descriptor(&self) -> &str {
        "android.gui.ISurfaceControl"
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        let mut reply = Parcel::new();
        reply.i32(EX_UNSUPPORTED_OPERATION);
        reply.string16(&format!("ISurfaceControl method {code} is not implemented"));
        reply.i32(0);
        Ok(reply.into_bytes().into())
    }
}

/// A producer identity is part of the surface-creation reply. No buffers are
/// queued through it until the Wayland compositor is implemented.
pub struct BufferProducer;

impl BinderObject for BufferProducer {
    fn descriptor(&self) -> &str {
        "android.gui.IGraphicBufferProducer"
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        let mut reply = Parcel::new();
        reply.i32(EX_UNSUPPORTED_OPERATION);
        reply.string16(&format!(
            "IGraphicBufferProducer method {code} is not implemented"
        ));
        reply.i32(0);
        Ok(reply.into_bytes().into())
    }
}

/// `android.gui.IDisplayEventConnection`, in declaration order.
mod event {
    pub const STEAL_RECEIVE_CHANNEL: u32 = 1;
    pub const SET_VSYNC_RATE: u32 = 2;
    pub const REQUEST_NEXT_VSYNC: u32 = 3;
}

/// What `createDisplayEventConnection` hands back.
///
/// The framework's `DisplayEventReceiver` takes this object, steals the receive
/// channel from it and then reads events off that channel. On a device the events
/// are vsyncs from the display hardware. Here the channel is real -- a socketpair,
/// which is what makes the receiver initialize instead of throwing -- and nothing
/// is written into it, because the frame clock belongs to the host's compositor
/// and reaching it is the windowing phase. A receiver that never hears anything is
/// the truth of that: no frames are presented by this side.
pub struct DisplayEventConnection {
    /// The end the caller reads from, handed over once.
    receive: Option<std::os::fd::OwnedFd>,
    /// The end this side would write events into. Kept so the channel has two
    /// ends, and unused until there is a frame clock to report.
    _send: std::os::fd::OwnedFd,
}

impl DisplayEventConnection {
    fn new() -> std::io::Result<Self> {
        use nix::sys::socket::{socketpair, AddressFamily, SockFlag, SockType};
        let (send, receive) = socketpair(
            AddressFamily::Unix,
            SockType::SeqPacket,
            None,
            SockFlag::SOCK_CLOEXEC,
        )?;
        Ok(Self {
            receive: Some(receive),
            _send: send,
        })
    }
}

impl BinderObject for DisplayEventConnection {
    fn descriptor(&self) -> &str {
        "android.gui.IDisplayEventConnection"
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        log::info!("DisplayEventConnection: code {}", code);
        match code {
            // `stealReceiveChannel(out BitTube outChannel)`: the reply is the
            // status, then the parcelable -- two descriptor words, the receive
            // channel first and the send channel second, in the order
            // `BitTube::writeToParcel` writes them.
            event::STEAL_RECEIVE_CHANNEL => {
                let mut reply = Parcel::new();
                // A parcelable argument is written with a presence word in front of
                // it -- the reader is `Parcel::readParcelable`, which reads one and
                // then hands the rest to `BitTube::readFromParcel`. Without it the
                // reader takes the descriptor word's type for the flag and reads
                // past the end of the reply, which is what `status=-61` (ENODATA)
                // from the framework was saying.
                reply.i32(1);
                let receive_at = reply.fd_placeholder();
                let send_at = reply.fd_placeholder();
                let receive = match self.receive.take() {
                    Some(fd) => fd,
                    // Stolen once, and the second caller gets nothing: the channel
                    // has one read end and it has been handed over.
                    None => return Ok(reply.into_bytes().into()),
                };
                let send = self._send.try_clone()?;
                Ok(Answer::from(reply.into_bytes())
                    .handing_fd(receive_at, receive)
                    .handing_fd(send_at, send))
            }
            // The rate is accepted and means nothing yet: there are no events to
            // rate-limit. `requestNextVsync` is oneway and likewise has nothing to
            // answer.
            event::SET_VSYNC_RATE | event::REQUEST_NEXT_VSYNC => {
                let reply = Parcel::new();
                Ok(reply.into_bytes().into())
            }
            other => {
                let mut reply = Parcel::new();
                reply.i32(EX_UNSUPPORTED_OPERATION);
                reply.string16(&format!(
                    "IDisplayEventConnection method {} is not implemented",
                    other
                ));
                reply.i32(0);
                Ok(reply.into_bytes().into())
            }
        }
    }
}

pub struct SurfaceFlinger {
    displays: Vec<Display>,
    /// Which interface this object reports itself as. Two of these are hosted, one
    /// per name, and each answers the code space its token names.
    interface: &'static str,
}

impl Default for SurfaceFlinger {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceFlinger {
    pub fn new() -> Self {
        Self::reporting(AIDL)
    }

    /// The object that reports this interface descriptor.
    pub fn reporting(interface: &'static str) -> Self {
        let displays = displays();
        for display in &displays {
            log::info!(
                "SurfaceFlinger: {} is {}x{} at {} Hz",
                display.connector,
                display.width,
                display.height,
                display.refresh_hz
            );
        }
        if displays.is_empty() {
            log::info!("SurfaceFlinger: no connected display in /sys/class/drm");
        }
        Self {
            displays,
            interface,
        }
    }

    fn display_for(&self, id: u64) -> Option<&Display> {
        self.displays.iter().find(|display| display.id == id)
    }

    /// The display a request names by token, or the primary one when the request
    /// names none (several methods take a `@nullable IBinder`).
    fn named_display(&self, args: &[u8]) -> Option<&Display> {
        let named = read_object_word(args).and_then(|word| token_display_id(&word));
        match named {
            Some(id) => self.display_for(id),
            None => self.displays.first(),
        }
    }

    fn refuse(&self, code: u32, interface: &str) -> Answer {
        let mut reply = Parcel::new();
        reply.i32(EX_UNSUPPORTED_OPERATION);
        reply.string16(&format!("{interface} method {code} is not implemented"));
        reply.i32(0); // the remote stack trace header the reader expects
        reply.into_bytes().into()
    }

    /// `ui::StaticDisplayInfo`, as `libs/ui/StaticDisplayInfo.cpp` writes it.
    fn static_display_info(&self, display: &Display) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&CONNECTION_TYPE_EXTERNAL.to_le_bytes());
        out.extend_from_slice(&display.density_dpi().to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes()); // not a secure display
                                                    // The monitor's product info lives in its EDID; nothing on this path reads
                                                    // it, and the field is optional, so it is absent rather than invented.
        out.extend_from_slice(&0i32.to_le_bytes()); // deviceProductInfo: none
        out.extend_from_slice(&ROTATION_0.to_le_bytes());
        out
    }

    /// `ui::DynamicDisplayInfo`, as `libs/ui/DynamicDisplayInfo.cpp` writes it.
    fn dynamic_display_info(&self, display: &Display) -> Vec<u8> {
        let mut out = Vec::new();
        // One mode: the host's preferred one. DRM's sysfs has the resolution and
        // nothing else, so the timings below are the ones a display that reports
        // no timing gets.
        out.extend_from_slice(&1u64.to_le_bytes()); // supportedDisplayModes: count
        out.extend_from_slice(&self.display_mode(display, 0));
        out.extend_from_slice(&0i32.to_le_bytes()); // activeDisplayModeId
        out.extend_from_slice(&1u64.to_le_bytes()); // supportedColorModes: count
        out.extend_from_slice(&COLOR_MODE_NATIVE.to_le_bytes());
        out.extend_from_slice(&COLOR_MODE_NATIVE.to_le_bytes()); // activeColorMode
                                                                 // HdrCapabilities: no types, and zero luminance means unknown.
        out.extend_from_slice(&0i32.to_le_bytes()); // supportedHdrTypes: count
        out.extend_from_slice(&0f32.to_le_bytes());
        out.extend_from_slice(&0f32.to_le_bytes());
        out.extend_from_slice(&0f32.to_le_bytes());
        out.push(0); // autoLowLatencyModeSupported
        out.push(0); // gameContentTypeSupported
        out.extend_from_slice(&0i32.to_le_bytes()); // preferredBootDisplayMode: the one mode
        out
    }

    /// `ui::DisplayMode`, as `libs/ui/DisplayMode.cpp` writes it in this branch:
    /// id, resolution, the two dpis, the refresh rate, three 64-bit offsets and
    /// the group. (Later branches split the refresh rate into peak and vsync; this
    /// one has the single `refreshRate` field.)
    fn display_mode(&self, display: &Display, id: i32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(display.width as i32).to_le_bytes());
        out.extend_from_slice(&(display.height as i32).to_le_bytes());
        out.extend_from_slice(&display.density_dpi().to_le_bytes()); // xDpi
        out.extend_from_slice(&display.density_dpi().to_le_bytes()); // yDpi
        out.extend_from_slice(&display.refresh_hz.to_le_bytes());
        out.extend_from_slice(&0i64.to_le_bytes()); // appVsyncOffset
        out.extend_from_slice(&0i64.to_le_bytes()); // sfVsyncOffset
        out.extend_from_slice(&0i64.to_le_bytes()); // presentationDeadline
        out.extend_from_slice(&0i32.to_le_bytes()); // group
        out
    }
}

/// The 28-byte object word at the start of a request's arguments, if the first
/// argument is one.
fn read_object_word(args: &[u8]) -> Option<[u8; 28]> {
    if args.len() < 28 {
        return None;
    }
    let mut word = [0u8; 28];
    word.copy_from_slice(&args[..28]);
    Some(word)
}

/// The display a token stands for. The broker writes the node into the cookie of
/// the object word it hands out, and the node carries this display's id.
fn token_display_id(word: &[u8; 28]) -> Option<u64> {
    let cookie = u64::from_le_bytes(word[16..24].try_into().ok()?);
    if cookie == 0 {
        return None;
    }
    Some(cookie)
}

impl BinderObject for SurfaceFlinger {
    fn descriptor(&self) -> &str {
        self.interface
    }

    fn transact(&mut self, code: u32, data: &[u8]) -> Result<Answer> {
        let interface = if carries_token(data, LEGACY) {
            "legacy"
        } else if carries_token(data, AIDL) {
            "aidl"
        } else {
            "unknown"
        };
        log::info!("SurfaceFlinger: {} code {}", interface, code);
        match interface {
            "legacy" => Ok(self.legacy_transact(code, args_after(data, LEGACY))),
            "aidl" => Ok(self.aidl_transact(code, args_after(data, AIDL))),
            _ => Ok(self.refuse(code, "an interface this is not")),
        }
    }
}

impl SurfaceFlinger {
    /// The older interface's methods, whose codes are the tags in
    /// `ISurfaceComposer.h` and whose replies are the sequences in
    /// `ISurfaceComposer.cpp`.
    fn legacy_transact(&mut self, code: u32, args: &[u8]) -> Answer {
        match code {
            // Nothing to report and nothing to do: the boot is not something a
            // userspace compositor waits on, and the framework only uses this to
            // let the compositor know it may start drawing.
            legacy::BOOT_FINISHED => Answer::default(),
            // A client connection is required before SurfaceComposerClient can
            // construct a SurfaceControl. It is an identity-only object here:
            // the framework's display setup can finish, while drawing remains
            // the windowing phase.
            legacy::CREATE_CONNECTION => {
                let mut reply = Parcel::new();
                let offset = reply.handle_binder(0);
                Answer::from(reply.into_bytes())
                    .handing(offset, Handed::fresh(Box::new(SurfaceComposerConnection)))
            }
            // A display event connection, whose receive channel is a socketpair.
            // Without one the framework's receiver fails to initialize and
            // `LocalDisplayAdapter` never connects the display at all -- the system
            // server then times out waiting for a default display. The channel is
            // real; nothing is written into it, because the frame clock is the
            // host compositor's.
            legacy::CREATE_DISPLAY_EVENT_CONNECTION => match DisplayEventConnection::new() {
                Ok(connection) => {
                    let mut reply = Parcel::new();
                    let offset = reply.handle_binder(0);
                    Answer::from(reply.into_bytes())
                        .handing(offset, Handed::fresh(Box::new(connection)))
                }
                Err(e) => {
                    log::warn!("SurfaceFlinger: no display event connection: {}", e);
                    let mut reply = Parcel::new();
                    reply.null_binder();
                    reply.into_bytes().into()
                }
            },
            legacy::GET_STATIC_DISPLAY_INFO => match self.named_display(args) {
                Some(display) => {
                    let info = self.static_display_info(display);
                    let mut reply = Parcel::new();
                    reply.i32(0);
                    // `Parcel::write(const Flattenable&)` writes the flattened size
                    // before the fields, and `Parcel::read` reads it back the same
                    // way. Without it the reader takes the first field for the size.
                    reply.i32(info.len() as i32);
                    reply.raw(&info);
                    reply.into_bytes().into()
                }
                None => {
                    let mut reply = Parcel::new();
                    reply.i32(-22); // NAME_NOT_FOUND
                    reply.into_bytes().into()
                }
            },
            legacy::GET_DYNAMIC_DISPLAY_INFO => match self.named_display(args) {
                Some(display) => {
                    let info = self.dynamic_display_info(display);
                    let mut reply = Parcel::new();
                    reply.i32(0);
                    reply.i32(info.len() as i32);
                    reply.raw(&info);
                    reply.into_bytes().into()
                }
                None => {
                    let mut reply = Parcel::new();
                    reply.i32(-22);
                    reply.into_bytes().into()
                }
            },
            // What the framework asks first, out of the `DisplayManagerService`
            // constructor: the color spaces it should compose in. sRGB for both
            // slots is not a placeholder -- the wide-gamut slot's value *means*
            // "no wide color gamut", which is what a host compositor gives an app.
            legacy::GET_COMPOSITION_PREFERENCE => {
                let mut reply = Parcel::new();
                reply.i32(0);
                reply.i32(DATASPACE_SRGB);
                reply.i32(PIXEL_FORMAT_RGBA_8888);
                reply.i32(DATASPACE_SRGB);
                reply.i32(PIXEL_FORMAT_RGBA_8888);
                reply.into_bytes().into()
            }
            // False, and it is the honest answer: this host does not compose
            // protected buffers, so nothing downstream should plan on it.
            legacy::GET_PROTECTED_CONTENT_SUPPORT => {
                let mut reply = Parcel::new();
                reply.i32(0);
                reply.boolean(false);
                reply.into_bytes().into()
            }
            // The primaries of the display. The monitor's own are in its EDID,
            // which is not readable here, so this is the sRGB set -- the standard
            // the desktop presents in and the one an app's content is authored in.
            legacy::GET_DISPLAY_NATIVE_PRIMARIES => {
                let mut reply = Parcel::new();
                reply.i32(0);
                for value in [
                    0.6400f32, 0.3300, // red
                    0.3000, 0.6000, // green
                    0.1500, 0.0600, // blue
                    0.3127, 0.3290, // white
                ] {
                    reply.raw(&value.to_le_bytes());
                }
                reply.into_bytes().into()
            }
            // The mode the display is running in, and the refresh rates it may use.
            // A host compositor chooses the mode; what this can honestly say is
            // that there is one mode and no group switching.
            // The reply has no status word of its own: the handler in
            // `ISurfaceComposer.cpp` writes `defaultMode` first and signals failure
            // by returning an error, so a result word here shifts every field after
            // it and the JNI's reader gives up with a null.
            legacy::GET_DESIRED_DISPLAY_MODE_SPECS => match self.named_display(args) {
                Some(display) => {
                    let mut reply = Parcel::new();
                    // `SurfaceControl.DesiredDisplayModeSpecs` has no status
                    // word: defaultMode, four float ranges, then the boolean.
                    reply.i32(0); // defaultMode: the one supported mode
                    for _ in 0..4 {
                        reply.raw(&display.refresh_hz.to_le_bytes());
                    }
                    reply.boolean(false); // allowGroupSwitching
                    reply.into_bytes().into()
                }
                None => {
                    let mut reply = Parcel::new();
                    reply.i32(-22);
                    reply.into_bytes().into()
                }
            },
            // No GPU context is created here, so the priority is the default one.
            legacy::GET_GPU_CONTEXT_PRIORITY => {
                let mut reply = Parcel::new();
                reply.i32(0);
                reply.into_bytes().into()
            }
            // How many buffers the compositor would want to acquire. There are no
            // surfaces yet, so this is the minimum a double-buffered pipeline
            // needs; the windowing phase is what will make it a real number.
            legacy::GET_MAX_ACQUIRED_BUFFER_COUNT => {
                let mut reply = Parcel::new();
                reply.i32(2);
                reply.into_bytes().into()
            }
            // Display decoration (rounded corners, a cutout) is a property of the
            // device's panel; a desktop monitor has none.
            legacy::GET_DISPLAY_DECORATION_SUPPORT => {
                let mut reply = Parcel::new();
                reply.boolean(false);
                reply.into_bytes().into()
            }
            other => self.refuse(other, LEGACY),
        }
    }

    /// The AIDL interface's methods, whose codes are its declaration order.
    fn aidl_transact(&mut self, code: u32, args: &[u8]) -> Answer {
        match code {
            aidl::GET_PHYSICAL_DISPLAY_IDS => {
                let mut reply = Parcel::new();
                reply.i32(self.displays.len() as i32);
                for display in &self.displays {
                    reply.i64(display.id as i64);
                }
                reply.into_bytes().into()
            }
            aidl::GET_PRIMARY_PHYSICAL_DISPLAY_ID => {
                let mut reply = Parcel::new();
                reply.i64(self.displays.first().map(|d| d.id as i64).unwrap_or(0));
                reply.into_bytes().into()
            }
            aidl::GET_PHYSICAL_DISPLAY_TOKEN => {
                let id = i64::from_le_bytes(
                    args.get(..8)
                        .and_then(|bytes| bytes.try_into().ok())
                        .unwrap_or([0; 8]),
                ) as u64;
                match self.display_for(id) {
                    Some(display) => {
                        let mut reply = Parcel::new();
                        let offset = reply.handle_binder(0);
                        let key = format!("{NAME}:token:{}", display.id);
                        Answer::from(reply.into_bytes()).handing(
                            offset,
                            Handed::new(key, Box::new(DisplayToken { id: display.id })),
                        )
                    }
                    // A display that is not there is a null token, which is what
                    // the interface's `@nullable` means.
                    None => {
                        let mut reply = Parcel::new();
                        reply.null_binder();
                        reply.into_bytes().into()
                    }
                }
            }
            aidl::GET_DISPLAY_STATE => {
                let mut reply = Parcel::new();
                reply.i32(match self.named_display(args) {
                    Some(_) => DISPLAY_STATE_ON,
                    None => 1, // OFF, for a display that is not there
                });
                reply.into_bytes().into()
            }
            // The media framework uses this to schedule video frames. The period is
            // the frame clock's; a host compositor's is not readable from here, so
            // it is the period of the refresh rate this side reports.
            aidl::GET_DISPLAY_STATS => {
                let display = self.named_display(args);
                let period = display
                    .map(|display| (1e9 / display.refresh_hz) as i64)
                    .unwrap_or(0);
                let mut reply = Parcel::new();
                reply.i64(0); // vsyncTime: no frame clock to report
                reply.i64(period);
                reply.into_bytes().into()
            }
            // Powering a display is the desktop's business: the host compositor
            // owns the panel, and a system server asking this to turn it off would
            // be asking for something that is not its to do. Accepted, not obeyed.
            aidl::SET_POWER_MODE => Answer::from(Vec::new()),
            aidl::CLEAR_BOOT_DISPLAY_MODE
            | aidl::SET_AUTO_LOW_LATENCY_MODE
            | aidl::SET_GAME_CONTENT_TYPE
            | aidl::NOTIFY_POWER_BOOST => Answer::from(Vec::new()),
            // False for both: the host has no boot-time display mode of its own, and
            // its output is sRGB rather than wide color.
            aidl::GET_BOOT_DISPLAY_MODE_SUPPORT => {
                let mut reply = Parcel::new();
                reply.boolean(false);
                reply.into_bytes().into()
            }
            aidl::IS_WIDE_COLOR_DISPLAY => {
                let mut reply = Parcel::new();
                reply.boolean(false);
                reply.into_bytes().into()
            }
            // Brightness belongs to the desktop, and the framework asks this first
            // precisely so it can leave it alone: false is the honest answer.
            aidl::GET_DISPLAY_BRIGHTNESS_SUPPORT => {
                let mut reply = Parcel::new();
                reply.boolean(false);
                reply.into_bytes().into()
            }
            // Virtual displays are the compositing half, which is not here.
            aidl::CREATE_DISPLAY | aidl::DESTROY_DISPLAY | aidl::SET_DISPLAY_BRIGHTNESS => {
                self.refuse(code, AIDL)
            }
            other => self.refuse(other, AIDL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::parcel::Reader;

    fn request(descriptor: &str, args: &[u8]) -> Vec<u8> {
        let mut data = vec![0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0xff];
        data.extend_from_slice(b"TSYS");
        let count = (descriptor.len() + 1) as i32;
        data.extend_from_slice(&count.to_le_bytes());
        for unit in descriptor.encode_utf16() {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        data.extend_from_slice(args);
        data
    }

    fn service() -> SurfaceFlinger {
        SurfaceFlinger {
            interface: AIDL,
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
    fn a_mode_line_is_read_as_a_size() {
        assert_eq!(mode_size("1920x1080"), Some((1920, 1080)));
        assert_eq!(mode_size("  3840x2160\n"), Some((3840, 2160)));
        assert_eq!(mode_size("garbage"), None);
    }

    #[test]
    fn the_aidl_codes_match_the_bundles_own_libgui() {
        // `getPhysicalDisplayIds` is 3 in the bundle's `BpSurfaceComposer`
        // disassembly, and third in the AIDL declaration order.
        assert_eq!(aidl::GET_PHYSICAL_DISPLAY_IDS, 3);
        assert_eq!(aidl::GET_PHYSICAL_DISPLAY_TOKEN, 5);
        assert_eq!(aidl::GET_DISPLAY_STATE, 8);
        assert_eq!(aidl::GET_DISPLAY_STATS, 7);
    }

    #[test]
    fn the_legacy_codes_are_the_tags_in_the_header() {
        // The positions of these names in `ISurfaceComposer.h`'s tag enum, counted
        // programmatically from the header. `getDynamicDisplayInfo` is 55 and the
        // framework's own request said so; the rest are the same count.
        assert_eq!(legacy::BOOT_FINISHED, 1);
        assert_eq!(legacy::CREATE_CONNECTION, 2);
        assert_eq!(legacy::GET_STATIC_DISPLAY_INFO, 3);
        assert_eq!(legacy::CREATE_DISPLAY_EVENT_CONNECTION, 4);
        assert_eq!(legacy::GET_COMPOSITION_PREFERENCE, 27);
        assert_eq!(legacy::GET_PROTECTED_CONTENT_SUPPORT, 32);
        assert_eq!(legacy::GET_DISPLAY_NATIVE_PRIMARIES, 34);
        assert_eq!(legacy::GET_DESIRED_DISPLAY_MODE_SPECS, 39);
        assert_eq!(legacy::GET_GPU_CONTEXT_PRIORITY, 53);
        assert_eq!(legacy::GET_MAX_ACQUIRED_BUFFER_COUNT, 54);
        assert_eq!(legacy::GET_DYNAMIC_DISPLAY_INFO, 55);
        assert_eq!(legacy::GET_DISPLAY_DECORATION_SUPPORT, 67);
    }

    #[test]
    fn display_ids_are_listed_and_the_primary_one_is_first() {
        let mut service = service();
        let reply = service
            .transact(aidl::GET_PHYSICAL_DISPLAY_IDS, &request(AIDL, &[]))
            .unwrap()
            .data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), 1); // one display
        assert_eq!(reader.i64(), 1 << 32);

        let reply = service
            .transact(aidl::GET_PRIMARY_PHYSICAL_DISPLAY_ID, &request(AIDL, &[]))
            .unwrap()
            .data;
        let mut reader = Reader::new(&reply);
        // The display id alone: no status in front of it.
        assert_eq!(reader.i64(), 1 << 32);
    }

    #[test]
    fn a_display_token_is_handed_back_as_an_object() {
        let mut service = service();
        let args = (1i64 << 32).to_le_bytes();
        let answer = service
            .transact(aidl::GET_PHYSICAL_DISPLAY_TOKEN, &request(AIDL, &args))
            .unwrap();
        // The status, then the object word the broker fills in.
        assert_eq!(answer.data.len(), 28);
        assert_eq!(answer.objects.len(), 1);
        assert_eq!(
            answer.objects[0].offset, 0,
            "the status word is the shim's, not the service's"
        );
        // Two calls for the same display give the same object, not two that mean
        // the same thing: the framework keeps the token and hands it back.
        let again = service
            .transact(aidl::GET_PHYSICAL_DISPLAY_TOKEN, &request(AIDL, &args))
            .unwrap();
        assert_eq!(again.objects.len(), 1);
    }

    #[test]
    fn a_display_that_is_not_there_gets_a_null_token() {
        let mut service = service();
        let args = 7i64.to_le_bytes();
        let answer = service
            .transact(aidl::GET_PHYSICAL_DISPLAY_TOKEN, &request(AIDL, &args))
            .unwrap();
        assert!(answer.objects.is_empty());
        // A null binder: the object word is there, and says it is nothing.
        assert_eq!(answer.data.len(), 28);
        assert_eq!(
            &answer.data[..4],
            &crate::binder::parcel::BINDER_TYPE_BINDER.to_le_bytes()
        );
        assert!(answer.data[4..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn the_composition_preference_is_srgb_and_says_so() {
        let mut service = service();
        let reply = service
            .transact(legacy::GET_COMPOSITION_PREFERENCE, &request(LEGACY, &[]))
            .unwrap()
            .data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), 0); // NO_ERROR, or the reader stops here
        assert_eq!(reader.i32(), DATASPACE_SRGB);
        assert_eq!(reader.i32(), PIXEL_FORMAT_RGBA_8888);
        // The wide-gamut slot is sRGB too, which is what "no wide color gamut"
        // means for this field.
        assert_eq!(reader.i32(), DATASPACE_SRGB);
        assert_eq!(reader.i32(), PIXEL_FORMAT_RGBA_8888);
    }

    #[test]
    fn the_static_display_info_has_the_fields_libui_writes() {
        let service = service();
        let display = &service.displays[0];
        let bytes = service.static_display_info(display);
        // connectionType, density, secure, deviceProductInfo (absent), rotation.
        assert_eq!(bytes.len(), 5 * 4);
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.i32(), CONNECTION_TYPE_EXTERNAL);
        assert_eq!(reader.f32(), DEFAULT_DENSITY_DPI);
        assert_eq!(reader.i32(), 0); // not secure
        assert_eq!(reader.i32(), 0); // no product info
        assert_eq!(reader.i32(), ROTATION_0);
    }

    #[test]
    fn the_dynamic_display_info_carries_one_mode_and_no_hdr() {
        let service = service();
        let display = &service.displays[0];
        let bytes = service.dynamic_display_info(display);
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.i64(), 1); // one supported mode, counted in eight bytes
        assert_eq!(reader.i32(), 0); // its id
        assert_eq!(reader.i32(), 1920);
        assert_eq!(reader.i32(), 1080);
        let _x_dpi = reader.i32();
        let _y_dpi = reader.i32();
        assert_eq!(reader.f32(), 60.0); // refreshRate
        assert_eq!(reader.i64(), 0);
        assert_eq!(reader.i64(), 0);
        assert_eq!(reader.i64(), 0);
        assert_eq!(reader.i32(), 0); // group
        assert_eq!(reader.i32(), 0); // activeDisplayModeId
        assert_eq!(reader.i64(), 1); // one color mode, counted in eight bytes
        assert_eq!(reader.i32(), COLOR_MODE_NATIVE);
        assert_eq!(reader.i32(), COLOR_MODE_NATIVE); // active
        assert_eq!(reader.i32(), 0); // no HDR types
        assert_eq!(reader.f32(), 0.0); // maxLuminance
        assert_eq!(reader.f32(), 0.0); // maxAverageLuminance
        assert_eq!(reader.f32(), 0.0); // minLuminance
        assert_eq!(reader.i8(), 0); // auto low latency, one byte
        assert_eq!(reader.i8(), 0); // game content type, one byte
        assert_eq!(reader.i32(), 0); // preferred boot mode: the one supported mode
    }

    #[test]
    fn a_display_event_connection_hands_over_a_channel() {
        let mut connection = DisplayEventConnection::new().unwrap();
        let answer = connection
            .transact(event::STEAL_RECEIVE_CHANNEL, &[])
            .unwrap();
        // The status, the presence word a parcelable argument carries, then two
        // descriptor objects: the receive channel and the send one, in the order
        // `BitTube::writeToParcel` writes them. A descriptor object is 24 bytes.
        assert_eq!(answer.data.len(), 4 + 24 + 24);
        assert_eq!(
            answer.objects.len(),
            0,
            "a descriptor is not a binder object"
        );
        assert_eq!(answer.fds.len(), 2);
        // The status and the presence word, then two 24-byte descriptor objects:
        // a descriptor carries no stability word, so the second lands at 32.
        assert_eq!(
            answer.fds[0].offset, 4,
            "the presence word, then the first descriptor"
        );
        assert_eq!(answer.fds[1].offset, 28);
        // A second steal gets nothing: the channel has one read end.
        let again = connection
            .transact(event::STEAL_RECEIVE_CHANNEL, &[])
            .unwrap();
        assert!(again.fds.is_empty());
    }

    #[test]
    fn a_request_for_the_other_interface_is_answered_from_the_other_code_space() {
        // Code 1 is `bootFinished` for the older interface and `createDisplay` for
        // the AIDL one. The token is what tells them apart -- without it, one of
        // the two would get the other's method.
        let mut service = service();
        let legacy = service.transact(1, &request(LEGACY, &[])).unwrap();
        assert!(legacy.data.is_empty(), "bootFinished answers nothing");

        let aidl = service.transact(1, &request(AIDL, &[])).unwrap();
        assert!(!aidl.data.is_empty(), "createDisplay is refused, by name");
        let mut reader = Reader::new(&aidl.data);
        assert_eq!(reader.i32(), EX_UNSUPPORTED_OPERATION);
    }

    #[test]
    fn desired_display_mode_specs_keep_the_native_field_order() {
        let mut service = service();
        let reply = service
            .transact(
                legacy::GET_DESIRED_DISPLAY_MODE_SPECS,
                &request(LEGACY, &[]),
            )
            .unwrap()
            .data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), 0);
        assert_eq!(reader.f32(), 60.0);
        assert_eq!(reader.f32(), 60.0);
        assert_eq!(reader.f32(), 60.0);
        assert_eq!(reader.f32(), 60.0);
        assert_eq!(reader.i32(), 0, "allowGroupSwitching is the final field");
    }
}
