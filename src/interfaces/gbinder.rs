// SPDX-License-Identifier: GPL-3.0-or-later

//! Android binder transport, backed by `libgbinder` loaded at runtime.
//!
//! The library is loaded with `dlopen` rather than linked, so the binary still
//! builds and runs on hosts without it and every call degrades to the same
//! "no container" behaviour instead of failing to start.
//!
//! NOTE: this layer is written against the libgbinder public headers but has
//! not been exercised against a running guest. Treat it as unverified until it
//! has been driven against a real container.

use libloading::Library;
use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void, CStr, CString};
use std::path::Path;
use std::sync::OnceLock;

/// `struct gbinder_reader { gconstpointer d[6]; }`
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GBinderReader {
    d: [*const c_void; 6],
}

impl GBinderReader {
    fn zeroed() -> Self {
        Self {
            d: [std::ptr::null(); 6],
        }
    }
}

/// `struct gbinder_writer { gconstpointer d[4]; }`
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GBinderWriter {
    d: [*const c_void; 4],
}

impl GBinderWriter {
    fn zeroed() -> Self {
        Self {
            d: [std::ptr::null(); 4],
        }
    }
}

type LocalTransactFunc = unsafe extern "C" fn(
    obj: *mut c_void,
    req: *mut c_void,
    code: c_uint,
    flags: c_uint,
    status: *mut c_int,
    user_data: *mut c_void,
) -> *mut c_void;

type PresenceFunc = unsafe extern "C" fn(user_data: *mut c_void);
type DeathFunc = unsafe extern "C" fn(user_data: *mut c_void);

type FnServicemanagerNew = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type FnServicemanagerNew2 =
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> *mut c_void;
type FnServicemanagerIsPresent = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnServicemanagerGetServiceSync =
    unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_int) -> *mut c_void;
type FnServicemanagerAddServiceSync =
    unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void) -> c_int;
type FnServicemanagerNewLocalObject =
    unsafe extern "C" fn(*mut c_void, *const c_char, LocalTransactFunc, *mut c_void) -> *mut c_void;
type FnServicemanagerListSync = unsafe extern "C" fn(*mut c_void) -> *mut *mut c_char;
type FnServicemanagerAddPresenceHandler =
    unsafe extern "C" fn(*mut c_void, PresenceFunc, *mut c_void) -> c_ulong;
type FnServicemanagerRemoveHandler = unsafe extern "C" fn(*mut c_void, c_ulong);
type FnServicemanagerUnref = unsafe extern "C" fn(*mut c_void);
type FnClientNew = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;
type FnClientNewRequest = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type FnClientTransactSyncReply =
    unsafe extern "C" fn(*mut c_void, c_uint, *mut c_void, *mut c_int) -> *mut c_void;
type FnClientUnref = unsafe extern "C" fn(*mut c_void);
type FnClientTransactSyncOneway = unsafe extern "C" fn(*mut c_void, c_uint, *mut c_void) -> c_int;
type FnRemoteObjectAddDeathHandler =
    unsafe extern "C" fn(*mut c_void, DeathFunc, *mut c_void) -> c_ulong;
type FnRemoteObjectRemoveHandler = unsafe extern "C" fn(*mut c_void, c_ulong);
type FnMainContextDefault = unsafe extern "C" fn() -> *mut c_void;
type FnMainContextIteration = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type FnLocalRequestInitWriter = unsafe extern "C" fn(*mut c_void, *mut GBinderWriter);
type FnLocalRequestUnref = unsafe extern "C" fn(*mut c_void);
type FnLocalObjectNewReply = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type FnLocalReplyInitWriter = unsafe extern "C" fn(*mut c_void, *mut GBinderWriter);
type FnLocalReplyUnref = unsafe extern "C" fn(*mut c_void);
type FnRemoteReplyInitReader = unsafe extern "C" fn(*mut c_void, *mut GBinderReader);
type FnRemoteReplyUnref = unsafe extern "C" fn(*mut c_void);
type FnRemoteRequestInitReader = unsafe extern "C" fn(*mut c_void, *mut GBinderReader);
type FnRemoteObjectUnref = unsafe extern "C" fn(*mut c_void);
type FnReaderReadInt32 = unsafe extern "C" fn(*mut GBinderReader, *mut i32) -> c_int;
type FnReaderReadInt64 = unsafe extern "C" fn(*mut GBinderReader, *mut i64) -> c_int;
type FnReaderReadString16 = unsafe extern "C" fn(*mut GBinderReader) -> *mut c_char;
type FnReaderReadBool = unsafe extern "C" fn(*mut GBinderReader, *mut c_int) -> c_int;
type FnReaderReadByte = unsafe extern "C" fn(*mut GBinderReader, *mut u8) -> c_int;
type FnReaderReadByteArray = unsafe extern "C" fn(*mut GBinderReader, *mut usize) -> *const c_void;
type FnReaderReadObject = unsafe extern "C" fn(*mut GBinderReader) -> *mut c_void;
type FnReaderAtEnd = unsafe extern "C" fn(*const GBinderReader) -> c_int;
type FnWriterAppendString16 = unsafe extern "C" fn(*mut GBinderWriter, *const c_char);
type FnWriterAppendInt32 = unsafe extern "C" fn(*mut GBinderWriter, u32);
type FnWriterAppendInt64 = unsafe extern "C" fn(*mut GBinderWriter, u64);
type FnWriterAppendBool = unsafe extern "C" fn(*mut GBinderWriter, c_int);
type FnWriterAppendInt8 = unsafe extern "C" fn(*mut GBinderWriter, i8);
type FnWriterAppendByteArray = unsafe extern "C" fn(*mut GBinderWriter, *const c_void, i32);
type FnWriterAppendRemoteObject = unsafe extern "C" fn(*mut GBinderWriter, *mut c_void);
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnStrfreev = unsafe extern "C" fn(*mut *mut c_char);

struct Api {
    _gbinder: Library,
    _glib: Library,
    g_free: FnFree,
    g_strfreev: FnStrfreev,
    servicemanager_new: FnServicemanagerNew,
    servicemanager_new2: FnServicemanagerNew2,
    servicemanager_is_present: FnServicemanagerIsPresent,
    servicemanager_get_service_sync: FnServicemanagerGetServiceSync,
    servicemanager_add_service_sync: FnServicemanagerAddServiceSync,
    servicemanager_new_local_object: FnServicemanagerNewLocalObject,
    servicemanager_list_sync: FnServicemanagerListSync,
    servicemanager_add_presence_handler: FnServicemanagerAddPresenceHandler,
    servicemanager_remove_handler: FnServicemanagerRemoveHandler,
    servicemanager_unref: FnServicemanagerUnref,
    client_new: FnClientNew,
    client_new_request: FnClientNewRequest,
    client_transact_sync_reply: FnClientTransactSyncReply,
    client_unref: FnClientUnref,
    client_transact_sync_oneway: FnClientTransactSyncOneway,
    local_request_init_writer: FnLocalRequestInitWriter,
    local_request_unref: FnLocalRequestUnref,
    local_object_new_reply: FnLocalObjectNewReply,
    local_reply_init_writer: FnLocalReplyInitWriter,
    local_reply_unref: FnLocalReplyUnref,
    remote_reply_init_reader: FnRemoteReplyInitReader,
    remote_reply_unref: FnRemoteReplyUnref,
    remote_request_init_reader: FnRemoteRequestInitReader,
    remote_object_unref: FnRemoteObjectUnref,
    remote_object_add_death_handler: FnRemoteObjectAddDeathHandler,
    remote_object_remove_handler: FnRemoteObjectRemoveHandler,
    main_context_default: FnMainContextDefault,
    main_context_iteration: FnMainContextIteration,
    reader_read_int32: FnReaderReadInt32,
    reader_read_int64: FnReaderReadInt64,
    reader_read_string16: FnReaderReadString16,
    reader_read_bool: FnReaderReadBool,
    reader_read_byte: FnReaderReadByte,
    reader_read_byte_array: FnReaderReadByteArray,
    reader_read_object: FnReaderReadObject,
    reader_at_end: FnReaderAtEnd,
    writer_append_string16: FnWriterAppendString16,
    writer_append_int32: FnWriterAppendInt32,
    writer_append_int64: FnWriterAppendInt64,
    writer_append_bool: FnWriterAppendBool,
    writer_append_int8: FnWriterAppendInt8,
    writer_append_byte_array: FnWriterAppendByteArray,
    writer_append_remote_object: FnWriterAppendRemoteObject,
}

impl Api {
    /// # Safety
    /// Resolving symbols and calling them is only sound while the loaded
    /// libraries stay mapped; both `Library` values are kept in `Api`.
    unsafe fn load() -> anyhow::Result<Self> {
        let gbinder = load_library(&["libgbinder.so.1", "libgbinder.so", "libgbinder-1.0.so.1"])?;
        let glib = load_library(&["libglib-2.0.so.0", "libglib-2.0.so"])?;

        macro_rules! gb {
            ($name:literal, $ty:ty) => {
                *gbinder.get::<$ty>(concat!($name, "\0").as_bytes())?
            };
        }

        Ok(Self {
            g_free: *glib.get::<FnFree>(b"g_free\0")?,
            g_strfreev: *glib.get::<FnStrfreev>(b"g_strfreev\0")?,
            servicemanager_new: gb!("gbinder_servicemanager_new", FnServicemanagerNew),
            servicemanager_new2: gb!("gbinder_servicemanager_new2", FnServicemanagerNew2),
            servicemanager_is_present: gb!(
                "gbinder_servicemanager_is_present",
                FnServicemanagerIsPresent
            ),
            servicemanager_get_service_sync: gb!(
                "gbinder_servicemanager_get_service_sync",
                FnServicemanagerGetServiceSync
            ),
            servicemanager_add_service_sync: gb!(
                "gbinder_servicemanager_add_service_sync",
                FnServicemanagerAddServiceSync
            ),
            servicemanager_new_local_object: gb!(
                "gbinder_servicemanager_new_local_object",
                FnServicemanagerNewLocalObject
            ),
            servicemanager_list_sync: gb!(
                "gbinder_servicemanager_list_sync",
                FnServicemanagerListSync
            ),
            servicemanager_add_presence_handler: gb!(
                "gbinder_servicemanager_add_presence_handler",
                FnServicemanagerAddPresenceHandler
            ),
            servicemanager_remove_handler: gb!(
                "gbinder_servicemanager_remove_handler",
                FnServicemanagerRemoveHandler
            ),
            servicemanager_unref: gb!("gbinder_servicemanager_unref", FnServicemanagerUnref),
            client_new: gb!("gbinder_client_new", FnClientNew),
            client_new_request: gb!("gbinder_client_new_request", FnClientNewRequest),
            client_transact_sync_reply: gb!(
                "gbinder_client_transact_sync_reply",
                FnClientTransactSyncReply
            ),
            client_unref: gb!("gbinder_client_unref", FnClientUnref),
            client_transact_sync_oneway: gb!(
                "gbinder_client_transact_sync_oneway",
                FnClientTransactSyncOneway
            ),
            local_request_init_writer: gb!(
                "gbinder_local_request_init_writer",
                FnLocalRequestInitWriter
            ),
            local_request_unref: gb!("gbinder_local_request_unref", FnLocalRequestUnref),
            local_object_new_reply: gb!("gbinder_local_object_new_reply", FnLocalObjectNewReply),
            local_reply_init_writer: gb!("gbinder_local_reply_init_writer", FnLocalReplyInitWriter),
            local_reply_unref: gb!("gbinder_local_reply_unref", FnLocalReplyUnref),
            remote_reply_init_reader: gb!(
                "gbinder_remote_reply_init_reader",
                FnRemoteReplyInitReader
            ),
            remote_reply_unref: gb!("gbinder_remote_reply_unref", FnRemoteReplyUnref),
            remote_request_init_reader: gb!(
                "gbinder_remote_request_init_reader",
                FnRemoteRequestInitReader
            ),
            remote_object_unref: gb!("gbinder_remote_object_unref", FnRemoteObjectUnref),
            remote_object_add_death_handler: gb!(
                "gbinder_remote_object_add_death_handler",
                FnRemoteObjectAddDeathHandler
            ),
            remote_object_remove_handler: gb!(
                "gbinder_remote_object_remove_handler",
                FnRemoteObjectRemoveHandler
            ),
            main_context_default: *glib.get::<FnMainContextDefault>(b"g_main_context_default\0")?,
            main_context_iteration: *glib
                .get::<FnMainContextIteration>(b"g_main_context_iteration\0")?,
            reader_read_int32: gb!("gbinder_reader_read_int32", FnReaderReadInt32),
            reader_read_int64: gb!("gbinder_reader_read_int64", FnReaderReadInt64),
            reader_read_string16: gb!("gbinder_reader_read_string16", FnReaderReadString16),
            reader_read_bool: gb!("gbinder_reader_read_bool", FnReaderReadBool),
            reader_read_byte: gb!("gbinder_reader_read_byte", FnReaderReadByte),
            reader_read_byte_array: gb!("gbinder_reader_read_byte_array", FnReaderReadByteArray),
            reader_read_object: gb!("gbinder_reader_read_object", FnReaderReadObject),
            reader_at_end: gb!("gbinder_reader_at_end", FnReaderAtEnd),
            writer_append_string16: gb!("gbinder_writer_append_string16", FnWriterAppendString16),
            writer_append_int32: gb!("gbinder_writer_append_int32", FnWriterAppendInt32),
            writer_append_int64: gb!("gbinder_writer_append_int64", FnWriterAppendInt64),
            writer_append_bool: gb!("gbinder_writer_append_bool", FnWriterAppendBool),
            writer_append_int8: gb!("gbinder_writer_append_int8", FnWriterAppendInt8),
            writer_append_byte_array: gb!(
                "gbinder_writer_append_byte_array",
                FnWriterAppendByteArray
            ),
            writer_append_remote_object: gb!(
                "gbinder_writer_append_remote_object",
                FnWriterAppendRemoteObject
            ),
            _gbinder: gbinder,
            _glib: glib,
        })
    }
}

unsafe fn load_library(names: &[&str]) -> anyhow::Result<Library> {
    let mut last = None;
    for name in names {
        match Library::new(name) {
            Ok(lib) => return Ok(lib),
            Err(e) => last = Some(e.to_string()),
        }
    }
    anyhow::bail!(
        "could not load {}: {}",
        names.join(", "),
        last.unwrap_or_default()
    )
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    // SAFETY: Api owns both libraries for the process lifetime, so the
    // resolved function pointers stay valid.
    API.get_or_init(|| unsafe { Api::load().ok() }).as_ref()
}

/// True when libgbinder could be loaded.
pub fn available() -> bool {
    api().is_some()
}

/// Run one iteration of the GLib default main context, which is where
/// libgbinder delivers presence and death notifications. Returns false when
/// no source was ready and `may_block` was false.
pub fn iterate_main_context(may_block: bool) -> bool {
    let Some(api) = api() else {
        // Without libgbinder there is nothing to pump; avoid a busy spin.
        std::thread::sleep(std::time::Duration::from_millis(100));
        return false;
    };
    // SAFETY: both calls are safe on the default context.
    unsafe {
        let context = (api.main_context_default)();
        (api.main_context_iteration)(context, may_block as c_int) != 0
    }
}

fn cstring(value: &str) -> anyhow::Result<CString> {
    CString::new(value).map_err(|_| anyhow::anyhow!("interior nul in {:?}", value))
}

fn take_string(api: &Api, ptr: *mut c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: libgbinder returned a NUL-terminated string we own.
    let s = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string();
    // SAFETY: the string was allocated by glib.
    unsafe { (api.g_free)(ptr as *mut c_void) };
    s
}

/// A transaction handler. It reads the request and fills the reply writer,
/// returning the binder status (0 on success).
type TransactHandler = Box<dyn Fn(Reader, u32, u32, &mut Writer) -> i32 + Send>;

/// A registered local-object handler, boxed and handed to libgbinder as
/// user_data. Reclaimed when the object is dropped.
struct Handler {
    callback: TransactHandler,
}

unsafe extern "C" fn transact_trampoline(
    obj: *mut c_void,
    req: *mut c_void,
    code: c_uint,
    flags: c_uint,
    status: *mut c_int,
    user_data: *mut c_void,
) -> *mut c_void {
    let Some(api) = api() else {
        if !status.is_null() {
            unsafe { *status = -1 };
        }
        return std::ptr::null_mut();
    };
    // SAFETY: user_data is the Box<Handler> passed to new_local_object.
    let handler = unsafe { &*(user_data as *const Handler) };

    let mut reader = GBinderReader::zeroed();
    // SAFETY: req is a valid GBinderRemoteRequest for this transaction.
    unsafe { (api.remote_request_init_reader)(req, &mut reader) };

    // SAFETY: obj is the GBinderLocalObject that received the transaction.
    let reply = unsafe { (api.local_object_new_reply)(obj) };
    if reply.is_null() {
        if !status.is_null() {
            unsafe { *status = -1 };
        }
        return std::ptr::null_mut();
    }
    let mut gb_writer = GBinderWriter::zeroed();
    // SAFETY: reply is a fresh local reply.
    unsafe { (api.local_reply_init_writer)(reply, &mut gb_writer) };

    let mut writer = Writer::for_reply(gb_writer);
    let tx_status = (handler.callback)(Reader { reader }, code, flags, &mut writer);

    if tx_status != 0 {
        if !status.is_null() {
            unsafe { *status = tx_status };
        }
        // SAFETY: reply is the object we created above.
        unsafe { (api.local_reply_unref)(reply) };
        return std::ptr::null_mut();
    }
    if !status.is_null() {
        unsafe { *status = 0 };
    }
    reply
}

pub struct ServiceManager {
    pub device: String,
    sm: Option<usize>,
}

// SAFETY: libgbinder objects are reference counted and internally locked, so
// handles can move between threads. The Python bindings shared them too.
unsafe impl Send for ServiceManager {}
unsafe impl Sync for ServiceManager {}

impl ServiceManager {
    pub fn new(
        device: &str,
        protocol: Option<&str>,
        binder_protocol: Option<&str>,
    ) -> anyhow::Result<Self> {
        if !Path::new(device).exists() {
            anyhow::bail!("Binder device {} not found", device);
        }
        let Some(api) = api() else {
            return Ok(Self {
                device: device.to_string(),
                sm: None,
            });
        };
        let device_c = cstring(device)?;
        // SAFETY: valid C strings; the returned pointer is owned.
        let sm = unsafe {
            match (protocol, binder_protocol) {
                (Some(sm_protocol), Some(rpc_protocol)) => {
                    let sm_c = cstring(sm_protocol)?;
                    let rpc_c = cstring(rpc_protocol)?;
                    (api.servicemanager_new2)(device_c.as_ptr(), sm_c.as_ptr(), rpc_c.as_ptr())
                }
                _ => (api.servicemanager_new)(device_c.as_ptr()),
            }
        };
        Ok(Self {
            device: device.to_string(),
            sm: if sm.is_null() {
                None
            } else {
                Some(sm as usize)
            },
        })
    }

    pub fn is_present(&self) -> bool {
        let (Some(api), Some(sm)) = (api(), self.sm) else {
            return Path::new(&self.device).exists();
        };
        // SAFETY: sm came from gbinder_servicemanager_new.
        unsafe { (api.servicemanager_is_present)(sm as *mut c_void) != 0 }
    }

    pub fn get_service_sync(&self, name: &str) -> anyhow::Result<Option<RemoteObject>> {
        let (Some(api), Some(sm)) = (api(), self.sm) else {
            return Ok(None);
        };
        let name_c = cstring(name)?;
        let mut status: c_int = 0;
        // SAFETY: valid manager and name. The returned object is autoreleased,
        // so take our own reference before it can be reclaimed.
        let obj = unsafe {
            (api.servicemanager_get_service_sync)(sm as *mut c_void, name_c.as_ptr(), &mut status)
        };
        if obj.is_null() || status != 0 {
            return Ok(None);
        }
        Ok(Some(RemoteObject {
            object: obj as usize,
            handlers: Vec::new(),
        }))
    }

    pub fn list_sync(&self) -> Vec<String> {
        let (Some(api), Some(sm)) = (api(), self.sm) else {
            return Vec::new();
        };
        // SAFETY: returns a NULL-terminated array of owned strings.
        let list = unsafe { (api.servicemanager_list_sync)(sm as *mut c_void) };
        if list.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut cursor = list;
        // SAFETY: walking the NULL-terminated array.
        unsafe {
            while !(*cursor).is_null() {
                out.push(CStr::from_ptr(*cursor).to_string_lossy().to_string());
                cursor = cursor.add(1);
            }
            (api.g_strfreev)(list);
        }
        out
    }

    pub fn add_service_sync(&self, name: &str, object: LocalObject) -> anyhow::Result<()> {
        let (Some(api), Some(sm)) = (api(), self.sm) else {
            return Ok(());
        };
        let name_c = cstring(name)?;
        // SAFETY: valid manager, name, and local object.
        let status = unsafe {
            (api.servicemanager_add_service_sync)(
                sm as *mut c_void,
                name_c.as_ptr(),
                object.object as *mut c_void,
            )
        };
        if status != 0 {
            anyhow::bail!("Failed to add service {}: {}", name, status);
        }
        Ok(())
    }

    pub fn new_local_object(
        &self,
        interface: &str,
        handler: impl Fn(Reader, u32, u32, &mut Writer) -> i32 + Send + 'static,
    ) -> LocalObject {
        let Some(api) = api() else {
            return LocalObject { object: 0 };
        };
        let Some(sm) = self.sm else {
            return LocalObject { object: 0 };
        };
        let Ok(interface_c) = cstring(interface) else {
            return LocalObject { object: 0 };
        };
        let boxed = Box::new(Handler {
            callback: Box::new(handler),
        });
        let handler_ptr = Box::into_raw(boxed);
        // SAFETY: passing a valid interface and our trampoline with the boxed
        // handler as user_data.
        let object = unsafe {
            (api.servicemanager_new_local_object)(
                sm as *mut c_void,
                interface_c.as_ptr(),
                transact_trampoline,
                handler_ptr as *mut c_void,
            )
        };
        if object.is_null() {
            // SAFETY: handler_ptr came from Box::into_raw above.
            unsafe { drop(Box::from_raw(handler_ptr)) };
            return LocalObject { object: 0 };
        }
        LocalObject {
            object: object as usize,
        }
    }

    pub fn add_presence_handler(&self, handler: impl Fn() + Send + 'static) -> u32 {
        let (Some(api), Some(sm)) = (api(), self.sm) else {
            return 0;
        };
        let boxed: Box<Box<dyn Fn() + Send>> = Box::new(Box::new(handler));
        let ptr = Box::into_raw(boxed);

        unsafe extern "C" fn trampoline(user_data: *mut c_void) {
            // SAFETY: user_data is the boxed closure from add_presence_handler.
            let f = unsafe { &*(user_data as *const Box<dyn Fn() + Send>) };
            f();
        }

        // SAFETY: valid manager and our trampoline.
        let id = unsafe {
            (api.servicemanager_add_presence_handler)(
                sm as *mut c_void,
                trampoline,
                ptr as *mut c_void,
            )
        };
        id as u32
    }

    pub fn remove_handler(&self, id: u32) {
        let (Some(api), Some(sm)) = (api(), self.sm) else {
            return;
        };
        // SAFETY: valid manager and handler id.
        unsafe {
            (api.servicemanager_remove_handler)(sm as *mut c_void, id as c_ulong);
        }
    }
}

impl Drop for ServiceManager {
    fn drop(&mut self) {
        if let (Some(api), Some(sm)) = (api(), self.sm) {
            // SAFETY: sm came from gbinder_servicemanager_new.
            unsafe { (api.servicemanager_unref)(sm as *mut c_void) };
        }
    }
}

pub struct RemoteObject {
    object: usize,
    /// Boxed death handlers, kept alive for as long as the object.
    handlers: Vec<Box<dyn Fn() + Send>>,
}

unsafe impl Send for RemoteObject {}
unsafe impl Sync for RemoteObject {}

impl RemoteObject {
    /// Register a death notification. The handler is kept alive with the
    /// object, mirroring the reference the bindings held.
    pub fn add_death_handler(&mut self, handler: impl Fn() + Send + 'static) -> u32 {
        let boxed: Box<Box<dyn Fn() + Send>> = Box::new(Box::new(handler));
        let ptr = Box::into_raw(boxed);

        unsafe extern "C" fn trampoline(user_data: *mut c_void) {
            // SAFETY: user_data is the boxed closure from add_death_handler.
            let f = unsafe { &*(user_data as *const Box<dyn Fn() + Send>) };
            f();
        }

        let mut id = 0u32;
        if let Some(api) = api() {
            // SAFETY: valid remote object and our trampoline.
            unsafe {
                id = (api.remote_object_add_death_handler)(
                    self.object as *mut c_void,
                    trampoline,
                    ptr as *mut c_void,
                ) as u32;
            }
        }
        // SAFETY: reclaim the Box<Box<..>> wrapper, keeping the inner closure
        // alive in `handlers`.
        let reclaimed = unsafe { *Box::from_raw(ptr) };
        self.handlers.push(reclaimed);
        id
    }

    /// Remove a death handler registered with `add_death_handler`.
    pub fn remove_death_handler(&self, id: u32) {
        if let Some(api) = api() {
            // SAFETY: valid remote object and handler id.
            unsafe {
                (api.remote_object_remove_handler)(self.object as *mut c_void, id as c_ulong);
            }
        }
    }
}

impl Drop for RemoteObject {
    fn drop(&mut self) {
        if let Some(api) = api() {
            // SAFETY: object came from the service manager.
            unsafe { (api.remote_object_unref)(self.object as *mut c_void) };
        }
    }
}

#[derive(Clone, Copy)]
pub struct LocalObject {
    object: usize,
}

unsafe impl Send for LocalObject {}
unsafe impl Sync for LocalObject {}

pub struct Client {
    pub remote: RemoteObject,
    pub interface: String,
    client: Option<usize>,
}

unsafe impl Send for Client {}
unsafe impl Sync for Client {}

impl Client {
    pub fn new(remote: RemoteObject, interface: &str) -> Self {
        let client = api().and_then(|api| {
            let interface_c = cstring(interface).ok()?;
            // SAFETY: valid remote object and interface string.
            let client =
                unsafe { (api.client_new)(remote.object as *mut c_void, interface_c.as_ptr()) };
            if client.is_null() {
                None
            } else {
                Some(client as usize)
            }
        });
        Self {
            remote,
            interface: interface.to_string(),
            client,
        }
    }

    pub fn new_request(&self) -> Writer {
        let resolved = api();
        let mut request = 0usize;
        let mut writer = GBinderWriter::zeroed();
        if let (Some(api), Some(client)) = (resolved, self.client) {
            // SAFETY: valid client; the request is owned and the writer points
            // into it.
            unsafe {
                let req = (api.client_new_request)(client as *mut c_void);
                if !req.is_null() {
                    (api.local_request_init_writer)(req, &mut writer);
                    request = req as usize;
                }
            }
        }
        Writer {
            request,
            writer,
            api: resolved,
            valid: request != 0,
        }
    }

    pub fn transact_sync_reply(&self, code: u32, request: Writer) -> anyhow::Result<(Reader, i32)> {
        let Some(api) = api() else {
            anyhow::bail!("Binder transact not available (no container)");
        };
        let Some(client) = self.client else {
            anyhow::bail!("Binder client not available (no service)");
        };
        if request.request == 0 {
            anyhow::bail!("Binder request not available");
        }
        let mut status: c_int = 0;
        // SAFETY: valid client, code, and request.
        let reply = unsafe {
            (api.client_transact_sync_reply)(
                client as *mut c_void,
                code,
                request.request as *mut c_void,
                &mut status,
            )
        };
        if reply.is_null() {
            anyhow::bail!("Binder transaction failed with status {}", status);
        }
        let mut reader = GBinderReader::zeroed();
        // SAFETY: reply is a fresh remote reply.
        unsafe { (api.remote_reply_init_reader)(reply, &mut reader) };
        // SAFETY: reply came from transact_sync_reply.
        unsafe { (api.remote_reply_unref)(reply) };
        Ok((Reader { reader }, status))
    }

    /// Fire a oneway transaction: no reply, returns the binder status.
    pub fn transact_sync_oneway(&self, code: u32, request: Writer) -> i32 {
        let (Some(api), Some(client)) = (api(), self.client) else {
            return -1;
        };
        if request.request == 0 {
            return -1;
        }
        // SAFETY: valid client, code, and request.
        unsafe {
            (api.client_transact_sync_oneway)(
                client as *mut c_void,
                code,
                request.request as *mut c_void,
            )
        }
    }

    pub fn interface(&self) -> &str {
        &self.interface
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let (Some(api), Some(client)) = (api(), self.client) {
            // SAFETY: client came from gbinder_client_new.
            unsafe { (api.client_unref)(client as *mut c_void) };
        }
    }
}

pub struct Writer {
    request: usize,
    writer: GBinderWriter,
    api: Option<&'static Api>,
    /// True once the writer has been initialised against a request or reply.
    valid: bool,
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let (Some(api), true) = (self.api, self.request != 0) {
            // SAFETY: request came from gbinder_client_new_request.
            unsafe { (api.local_request_unref)(self.request as *mut c_void) };
        }
    }
}

impl Writer {
    /// Wrap a writer initialised on a local reply. Replies are owned by the
    /// trampoline, so there is nothing to unref here.
    pub(crate) fn for_reply(writer: GBinderWriter) -> Self {
        Self {
            request: 0,
            writer,
            api: api(),
            valid: true,
        }
    }

    fn writer_ptr(&mut self) -> *mut GBinderWriter {
        &mut self.writer
    }

    pub fn append_string16(&mut self, s: &str) {
        let (Some(api), Ok(s)) = (self.api, cstring(s)) else {
            return;
        };
        if !self.valid {
            return;
        }
        let writer = self.writer_ptr();
        // SAFETY: writer points into the live request.
        unsafe { (api.writer_append_string16)(writer, s.as_ptr()) };
    }

    pub fn append_int32(&mut self, v: i32) {
        let Some(api) = self.api else { return };
        if !self.valid {
            return;
        }
        let writer = self.writer_ptr();
        // SAFETY: writer points into the live request.
        unsafe { (api.writer_append_int32)(writer, v as u32) };
    }

    pub fn append_int64(&mut self, v: i64) {
        let Some(api) = self.api else { return };
        if !self.valid {
            return;
        }
        let writer = self.writer_ptr();
        // SAFETY: writer points into the live request.
        unsafe { (api.writer_append_int64)(writer, v as u64) };
    }

    pub fn append_bool(&mut self, v: bool) {
        let Some(api) = self.api else { return };
        if !self.valid {
            return;
        }
        let writer = self.writer_ptr();
        // SAFETY: writer points into the live request.
        unsafe { (api.writer_append_bool)(writer, v as c_int) };
    }

    pub fn append_byte(&mut self, v: u8) {
        let Some(api) = self.api else { return };
        if !self.valid {
            return;
        }
        let writer = self.writer_ptr();
        // SAFETY: writer points into the live request.
        unsafe { (api.writer_append_int8)(writer, v as i8) };
    }

    pub fn append_byte_array(&mut self, data: &[u8]) {
        let Some(api) = self.api else { return };
        if !self.valid {
            return;
        }
        let writer = self.writer_ptr();
        // SAFETY: writer points into the live request; libgbinder copies the
        // data before returning.
        unsafe {
            (api.writer_append_byte_array)(
                writer,
                data.as_ptr() as *const c_void,
                data.len() as i32,
            )
        };
    }

    pub fn append_object(&mut self, object: Option<&RemoteObject>) {
        let Some(api) = self.api else { return };
        if !self.valid {
            return;
        }
        let ptr = object
            .map(|o| o.object as *mut c_void)
            .unwrap_or(std::ptr::null_mut());
        let writer = self.writer_ptr();
        // SAFETY: writer points into the live request.
        unsafe { (api.writer_append_remote_object)(writer, ptr) };
    }
}

pub struct Reader {
    reader: GBinderReader,
}

impl Reader {
    pub fn init_reader(_data: &[u8]) -> Self {
        Self {
            reader: GBinderReader::zeroed(),
        }
    }

    pub fn at_end(&self) -> bool {
        match api() {
            // SAFETY: reader is valid for this reader's lifetime.
            Some(api) => unsafe { (api.reader_at_end)(&self.reader) != 0 },
            None => true,
        }
    }

    pub fn read_int32(&mut self) -> anyhow::Result<(i32, i32)> {
        let api = api().ok_or_else(|| anyhow::anyhow!("No data"))?;
        let mut value: i32 = 0;
        // SAFETY: reads into a local and advances the reader.
        let ok = unsafe { (api.reader_read_int32)(&mut self.reader, &mut value) };
        if ok == 0 {
            anyhow::bail!("No data");
        }
        Ok((0, value))
    }

    pub fn read_int64(&mut self) -> anyhow::Result<(i32, i64)> {
        let api = api().ok_or_else(|| anyhow::anyhow!("No data"))?;
        let mut value: i64 = 0;
        // SAFETY: reads into a local and advances the reader.
        let ok = unsafe { (api.reader_read_int64)(&mut self.reader, &mut value) };
        if ok == 0 {
            anyhow::bail!("No data");
        }
        Ok((0, value))
    }

    pub fn read_string16(&mut self) -> anyhow::Result<String> {
        let api = api().ok_or_else(|| anyhow::anyhow!("No data"))?;
        // SAFETY: returns an owned string we free.
        let ptr = unsafe { (api.reader_read_string16)(&mut self.reader) };
        if ptr.is_null() {
            anyhow::bail!("No data");
        }
        Ok(take_string(api, ptr))
    }

    pub fn read_bool(&mut self) -> anyhow::Result<(i32, bool)> {
        let api = api().ok_or_else(|| anyhow::anyhow!("No data"))?;
        let mut value: c_int = 0;
        // SAFETY: reads into a local and advances the reader.
        let ok = unsafe { (api.reader_read_bool)(&mut self.reader, &mut value) };
        if ok == 0 {
            anyhow::bail!("No data");
        }
        Ok((0, value != 0))
    }

    pub fn read_byte(&mut self) -> anyhow::Result<(i32, u8)> {
        let api = api().ok_or_else(|| anyhow::anyhow!("No data"))?;
        let mut value: u8 = 0;
        // SAFETY: reads into a local and advances the reader.
        let ok = unsafe { (api.reader_read_byte)(&mut self.reader, &mut value) };
        if ok == 0 {
            anyhow::bail!("No data");
        }
        Ok((0, value))
    }

    pub fn read_byte_array(&mut self) -> anyhow::Result<Vec<u8>> {
        let api = api().ok_or_else(|| anyhow::anyhow!("No data"))?;
        let mut len: usize = 0;
        // SAFETY: returns a borrowed pointer plus length.
        let ptr = unsafe { (api.reader_read_byte_array)(&mut self.reader, &mut len) };
        if ptr.is_null() {
            anyhow::bail!("No data");
        }
        // SAFETY: the pointer is valid for len bytes per libgbinder.
        let slice = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
        Ok(slice.to_vec())
    }

    pub fn read_object(&mut self) -> anyhow::Result<Option<RemoteObject>> {
        let api = api().ok_or_else(|| anyhow::anyhow!("No data"))?;
        // SAFETY: returns an autoreleased remote object.
        let ptr = unsafe { (api.reader_read_object)(&mut self.reader) };
        if ptr.is_null() {
            return Ok(None);
        }
        Ok(Some(RemoteObject {
            object: ptr as usize,
            handlers: Vec::new(),
        }))
    }
}

/// Publish a binder service and keep it registered while `stop` is false,
/// pumping the GLib main context so presence notifications are delivered.
///
/// This is the shared body of every `add_service` in the interfaces.
pub fn serve(
    args: &crate::args::MosaicArgs,
    interface: &str,
    service_name: &str,
    handler: impl Fn(Reader, u32, u32, &mut Writer) -> i32 + Send + 'static,
    stop: &std::sync::atomic::AtomicBool,
) {
    use std::sync::atomic::Ordering;

    let Ok((binder, _, _)) = load_binder_nodes(args) else {
        return;
    };
    let cfg = crate::config::load(&args.config);
    let sm_protocol = cfg.mosaic.get("service_manager_protocol").cloned();
    let binder_protocol = cfg.mosaic.get("binder_protocol").cloned();
    let device = format!("/dev/{}", binder);
    let Ok(sm) = ServiceManager::new(&device, sm_protocol.as_deref(), binder_protocol.as_deref())
    else {
        log::debug!("Failed to create ServiceManager for {}", device);
        return;
    };

    let object = sm.new_local_object(interface, handler);
    let registration = (service_name.to_string(), object);

    if sm.is_present() {
        if let Err(e) = sm.add_service_sync(&registration.0, registration.1) {
            log::error!("Failed to add service {}: {}", registration.0, e);
        }
    }

    // The presence handler outlives this call, so the manager needs a static
    // home. Services run for the process lifetime.
    let sm: &'static ServiceManager = Box::leak(Box::new(sm));
    let _handler = sm.add_presence_handler(move || {
        if !sm.is_present() {
            return;
        }
        if let Err(e) = sm.add_service_sync(&registration.0, registration.1) {
            log::error!("Failed to add service {}: {}", registration.0, e);
        }
    });
    let _ = _handler;

    while !stop.load(Ordering::SeqCst) {
        iterate_main_context(true);
    }
}

pub fn load_binder_nodes(
    args: &crate::args::MosaicArgs,
) -> anyhow::Result<(String, String, String)> {
    let cfg = crate::config::load(&args.config);
    let binder = cfg
        .mosaic
        .get("binder")
        .cloned()
        .unwrap_or_else(|| "binder".to_string());
    let vnd = cfg
        .mosaic
        .get("vndbinder")
        .cloned()
        .unwrap_or_else(|| "vndbinder".to_string());
    let hw = cfg
        .mosaic
        .get("hwbinder")
        .cloned()
        .unwrap_or_else(|| "hwbinder".to_string());
    Ok((binder, vnd, hw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_writer_sizes_match_the_c_headers() {
        // gconstpointer d[6] and d[4] are the documented public layouts.
        assert_eq!(
            std::mem::size_of::<GBinderReader>(),
            6 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::size_of::<GBinderWriter>(),
            4 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn new_fails_cleanly_when_device_missing() {
        assert!(ServiceManager::new("/dev/definitely-not-binder", None, None).is_err());
    }

    #[test]
    fn reader_reports_no_data_without_a_transaction() {
        let mut reader = Reader::init_reader(&[]);
        assert!(reader.read_int32().is_err());
        assert!(reader.at_end());
    }
}
