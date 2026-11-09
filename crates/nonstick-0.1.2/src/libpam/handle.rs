use super::conversation::{OwnedConversation, PamConv};
use crate::_doc::{guide, linklist, man7, stdlinks};
use crate::constants::{ErrorCode, RawFlags, Result, ReturnCode};
use crate::conv::Exchange;
use crate::environ::EnvironMapMut;
use crate::handle::PamShared;
use crate::items::{Items, ItemsMut};
use crate::libpam::environ::{LibPamEnviron, LibPamEnvironMut};
use crate::libpam::items::{ItemType, LibPamItems, LibPamItemsMut};
use crate::libpam::{items, memory};
use crate::logging::{Level, Location, Logger};
use crate::{AuthnFlags, AuthtokFlags, Conversation, EnvironMap, ModuleClient, Transaction};
use std::any::TypeId;
use std::cell::Cell;
use std::ffi::{c_char, c_int, c_void, CString, OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::ptr::NonNull;
use std::{any, fmt, ptr};

/// An owned PAM handle.
pub struct LibPamTransaction<C: Conversation> {
    /// The handle itself.  We guarantee this will not be null.
    handle: *mut LibPamHandle,
    /// The last return value from the handle.
    last_return: Cell<Result<()>>,
    /// If set, the Conversation that this PAM handle owns.
    ///
    /// We have to hold on to this because the PAM specification doesn't
    /// actually say what the PAM library should do with a passed-in
    /// conversation. Linux-PAM copies the contents of the `pam_conv` struct
    /// that you pass in to `pam_start`, but OpenPAM uses the pointer itself,
    /// so you have to keep it in one place.
    conversation: Box<OwnedConversation<C>>,
}

impl<C: Conversation> fmt::Debug for LibPamTransaction<C> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct(any::type_name::<Self>())
            .field("handle", &format!("{:p}", self.handle))
            .field("last_return", &self.last_return.get())
            .field("conversation", &format!("{:p}", self.conversation))
            .finish()
    }
}

/// Builder to start a [`LibPamTransaction`].
///
/// Use [`Self::new_with_service`] to build a new PAM transaction.
#[derive(Debug, PartialEq)]
pub struct TransactionBuilder {
    service_name: OsString,
    username: Option<OsString>,
}

impl TransactionBuilder {
    /// Creates a builder to start a PAM transaction for the given service.
    ///
    /// The service name is what controls the steps and checks PAM goes through
    /// when authenticating a user. This corresponds to the configuration file
    /// usually at <code>/etc/pam.d/<var>service_name</var></code>.
    ///
    /// You usually want to call [`username`](Self::username) to set
    /// the username before starting the transaction.
    ///
    /// # References
    #[doc = linklist!(pam_start: adg, _std)]
    ///
    #[doc = stdlinks!(3 pam_start)]
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_start")]
    pub fn new_with_service(service_name: impl AsRef<OsStr>) -> Self {
        Self {
            service_name: service_name.as_ref().into(),
            username: None,
        }
    }

    /// Updates the service name.
    pub fn service_name(mut self, service_name: impl AsRef<OsStr>) -> Self {
        self.service_name = service_name.as_ref().into();
        self
    }

    /// Sets the username. Setting this will avoid the need for an extra
    /// round trip through the conversation and may otherwise improve
    /// the login experience.
    pub fn username(mut self, username: impl AsRef<OsStr>) -> Self {
        self.username = Some(username.as_ref().into());
        self
    }

    /// Builds the PAM handle and starts the transaction.
    pub fn build<C: Conversation>(self, conv: C) -> Result<LibPamTransaction<C>> {
        LibPamTransaction::start(self.service_name, self.username, conv)
    }
}

impl<C: Conversation> LibPamTransaction<C> {
    fn start(service_name: OsString, username: Option<OsString>, conversation: C) -> Result<Self> {
        let mut conv = Box::new(OwnedConversation::new(conversation));
        let service_cstr = CString::new(service_name.as_bytes()).expect("null is forbidden");
        let username_cstr = memory::option_cstr_os(username.as_deref());
        let username_cstr = memory::prompt_ptr(username_cstr.as_deref());

        let mut handle: *mut libpam_sys::pam_handle = ptr::null_mut();
        let conv_ptr: *mut OwnedConversation<_> = conv.as_mut() as _;
        // SAFETY: We've set everything up properly to call `pam_start`.
        // The returned value will be a valid pointer provided the result is OK.
        let result = unsafe {
            libpam_sys::pam_start(
                service_cstr.as_ptr(),
                username_cstr,
                conv_ptr.cast(),
                &mut handle,
            )
        };
        ErrorCode::result_from(result)?;
        let handle = NonNull::new(handle).ok_or(ErrorCode::BufferError)?;
        Ok(Self {
            handle: handle.as_ptr().cast(),
            last_return: Cell::new(Ok(())),
            conversation: conv,
        })
    }

    #[cfg_attr(
        pam_impl = "LinuxPam",
        doc = "Ends the PAM transaction \"quietly\" (on Linux-PAM only)."
    )]
    #[cfg_attr(
        not(pam_impl = "LinuxPam"),
        doc = "Exactly equivalent to `drop(self)` (except on Linux-PAM)."
    )]
    ///
    /// On Linux-PAM, this is equivalent to passing the `PAM_DATA_SILENT` flag
    /// to [`pam_end` on Linux-PAM][man7], which signals that data cleanup
    /// should "not treat the call too seriously" \[sic].
    ///
    /// On other platforms, this is no different than letting the transaction
    /// end on its own.
    #[doc = ""]
    #[doc = man7!(3 pam_end)]
    pub fn end_silent(self) {
        #[cfg(pam_impl = "LinuxPam")]
        {
            let mut me = std::mem::ManuallyDrop::new(self);
            me.end_internal(libpam_sys::PAM_DATA_SILENT);
        }
        // If it's not LinuxPam, we just drop normally.
    }

    /// Internal "end" function, which binary-ORs the status with `or_with`.
    fn end_internal(&mut self, or_with: c_int) {
        let last: c_int = ReturnCode::from(self.last_return.get()).into();
        let result = last | or_with;
        unsafe { libpam_sys::pam_end(self.handle.cast(), result) };
    }
}

macro_rules! wrap {
    (fn $name:ident($ftype:ident) { $pam_func:ident }) => {
        fn $name(&mut self, flags: $ftype) -> Result<()> {
            let flags: RawFlags = flags.into();
            ErrorCode::result_from(unsafe {
                libpam_sys::$pam_func((self as *mut Self).cast(), flags.into())
            })
        }
    };
}

impl Transaction for LibPamHandle {
    wrap!(fn authenticate(AuthnFlags) { pam_authenticate });
    wrap!(fn account_management(AuthnFlags) { pam_acct_mgmt });
    wrap!(fn change_authtok(AuthtokFlags) { pam_chauthtok });
}

// TODO: pam_setcred - app
//       pam_open_session - app
//       pam_close_session - app

impl<C: Conversation> Drop for LibPamTransaction<C> {
    /// Closes the PAM session on an owned PAM handle.
    ///
    /// This internally calls `pam_end` with the appropriate error code.
    ///
    /// # References
    #[doc = linklist!(pam_end: adg, _std)]
    ///
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_end")]
    #[doc = stdlinks!(3 pam_end)]
    fn drop(&mut self) {
        self.end_internal(0)
    }
}

macro_rules! delegate {
    // First have the kind that save the result after delegation.
    (fn $meth:ident(&self $(, $param:ident: $typ:ty)*) -> Result<$ret:ty>) => {
        fn $meth(&self $(, $param: $typ)*) -> Result<$ret> {
            let result = unsafe { &*self.handle }.$meth($($param),*);
            self.last_return.set(split(&result));
            result
        }
    };
    (fn $meth:ident(&mut self $(, $param:ident: $typ:ty)*) -> Result<$ret:ty>) => {
        fn $meth(&mut self $(, $param: $typ)*) -> Result<$ret> {
            let result = unsafe { &mut *self.handle }.$meth($($param),*);
            self.last_return.set(split(&result));
            result
        }
    };
    // Then have the kind that are just raw delegates
    (fn $meth:ident(&self $(, $param:ident: $typ:ty)*) -> $ret:ty) => {
        fn $meth(&self $(, $param: $typ)*) -> $ret {
            unsafe { &*self.handle }.$meth($($param),*)
        }
    };
    (fn $meth:ident(&mut self $(, $param:ident: $typ:ty)*) -> $ret:ty) => {
        fn $meth(&mut self $(, $param: $typ)*) -> $ret {
            unsafe { &mut *self.handle }.$meth($($param),*)
        }
    };
    // Then have item getters / setters
    (get = $get:ident$(, set = $set:ident)?) => {
        delegate!(fn $get(&self) -> Result<Option<OsString>>);
        $(delegate!(set = $set);)?
    };
    (set = $set:ident) => {
        delegate!(fn $set(&mut self, value: Option<&OsStr>) -> Result<()>);
    };
}

fn split<T>(result: &Result<T>) -> Result<()> {
    result.as_ref().map(drop).map_err(|&e| e)
}

impl<C: Conversation> Logger for LibPamTransaction<C> {
    delegate!(fn log(&self, level: Level, location: Location<'_>, entry: fmt::Arguments) -> ());
}

impl<C: Conversation> Transaction for LibPamTransaction<C> {
    delegate!(fn authenticate(&mut self, flags: AuthnFlags) -> Result<()>);
    delegate!(fn account_management(&mut self, flags: AuthnFlags) -> Result<()>);
    delegate!(fn change_authtok(&mut self, flags: AuthtokFlags) -> Result<()>);
}

impl<C: Conversation> PamShared for LibPamTransaction<C> {
    delegate!(fn environ(&self) -> impl EnvironMap);
    delegate!(fn environ_mut(&mut self) -> impl EnvironMapMut);
    delegate!(fn items(&self) -> impl Items);
    delegate!(fn items_mut(&mut self) -> impl ItemsMut);
}

/// An owned variation of a basic PAM handle.
///
/// This is the most basic version of a wrapped PAM handle. It's mostly used
/// as the inside of the [`LibPamTransaction`], but can also be used to "adopt"
/// a PAM handle created by another library.
///
/// If [`Self::end`] is not called, this will always call `pam_end` reporting
/// successful completion.
#[repr(transparent)]
pub struct LibPamHandle(libpam_sys::pam_handle);

impl LibPamHandle {
    /// Ends the transaction, reporting `error_code` to cleanup callbacks.
    ///
    /// # References
    #[doc = linklist!(pam_end: adg, _std)]
    ///
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_end")]
    #[doc = stdlinks!(3 pam_end)]
    pub fn end(&mut self, result: Result<()>) {
        let code: ReturnCode = result.into();
        unsafe { libpam_sys::pam_end(self.inner_mut(), code.into()) };
    }

    #[cfg_attr(
        not(pam_impl = "LinuxPam"),
        doc = "Exactly equivalent to [`Self::end`], except on Linux-PAM."
    )]
    #[cfg_attr(
        pam_impl = "LinuxPam",
        doc = "Ends the transaction \"quietly\", reporting the given result."
    )]
    ///
    /// On Linux-PAM only, this sets the `PAM_DATA_SILENT` flag on the value
    /// passed to the cleanup callbacks. This conventionally means that this
    /// `pam_end` call is occurring on a forked process, and that a session
    /// may still be open on the parent process, and modules "should not treat
    /// the call too seriously".
    ///
    /// # References
    #[doc = linklist!(pam_end: adg, _std)]
    ///
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_end")]
    #[doc = stdlinks!(3 pam_end)]
    pub fn end_silent(&mut self, result: Result<()>) {
        let result: c_int = ReturnCode::from(result).into();
        #[cfg(pam_impl = "LinuxPam")]
        let result = result | libpam_sys::PAM_DATA_SILENT;
        unsafe {
            libpam_sys::pam_end(self.inner_mut(), result);
        }
    }

    /// Gets a reference to the inner PAM handle.
    pub fn inner(&self) -> &libpam_sys::pam_handle {
        &self.0
    }
    /// Gets a mutable reference to the inner PAM handle.
    pub fn inner_mut(&mut self) -> &mut libpam_sys::pam_handle {
        &mut self.0
    }
}

impl Logger for LibPamHandle {
    fn log(&self, level: Level, loc: Location<'_>, entry: fmt::Arguments) {
        let entry = match CString::new(entry.to_string()).ok() {
            Some(e) => e,
            None => return,
        };
        #[cfg(any(pam_impl = "LinuxPam", pam_impl = "Sun"))]
        {
            let level = match level {
                Level::Error => libc::LOG_ERR,
                Level::Warn => libc::LOG_WARNING,
                Level::Info => libc::LOG_INFO,
                Level::Debug => libc::LOG_DEBUG,
            };
            _ = loc;
            // SAFETY: We're calling this function with a known value.
            #[cfg(pam_impl = "LinuxPam")]
            unsafe {
                libpam_sys::pam_syslog(self.inner(), level, b"%s\0".as_ptr().cast(), entry.as_ptr())
            }
            #[cfg(pam_impl = "Sun")]
            unsafe {
                libpam_sys::__pam_log(level, b"%s\0".as_ptr().cast(), entry.as_ptr())
            }
        }
        #[cfg(pam_impl = "OpenPam")]
        {
            let func = CString::new(loc.function).unwrap_or(CString::default());
            let level = match level {
                Level::Error => libpam_sys::PAM_LOG_ERROR,
                Level::Warn => libpam_sys::PAM_LOG_NOTICE,
                Level::Info => libpam_sys::PAM_LOG_VERBOSE,
                Level::Debug => libpam_sys::PAM_LOG_DEBUG,
            };
            // SAFETY: We're calling this function with a known value.
            unsafe {
                libpam_sys::_openpam_log(
                    level as c_int,
                    func.as_ptr(),
                    b"%s\0".as_ptr().cast(),
                    entry.as_ptr(),
                )
            }
        }
    }
}

impl PamShared for LibPamHandle {
    fn environ(&self) -> impl EnvironMap {
        LibPamEnviron::new(self)
    }

    fn environ_mut(&mut self) -> impl EnvironMapMut {
        LibPamEnvironMut::new(self)
    }

    fn items(&self) -> impl Items {
        LibPamItems(self)
    }

    fn items_mut(&mut self) -> impl ItemsMut {
        LibPamItemsMut(self)
    }
}

impl Conversation for LibPamHandle {
    fn communicate(&self, messages: &[Exchange]) {
        match self.conversation_item() {
            Ok(conv) => conv.communicate(messages),
            Err(e) => {
                for msg in messages {
                    msg.set_error(e)
                }
            }
        }
    }
}

impl ModuleClient for LibPamHandle {
    fn username(&mut self, prompt: Option<&OsStr>) -> Result<OsString> {
        let prompt = memory::option_cstr_os(prompt);
        let mut output: *const c_char = ptr::null();
        let ret = unsafe {
            libpam_sys::pam_get_user(
                self.inner_mut(),
                &mut output,
                memory::prompt_ptr(prompt.as_deref()),
            )
        };
        ErrorCode::result_from(ret)?;
        Ok(unsafe { memory::copy_pam_string(output).ok_or(ErrorCode::ConversationError)? })
    }

    fn authtok(&mut self, prompt: Option<&OsStr>) -> Result<OsString> {
        self.get_authtok(prompt, ItemType::AuthTok)
    }

    fn old_authtok(&mut self, prompt: Option<&OsStr>) -> Result<OsString> {
        self.get_authtok(prompt, ItemType::OldAuthTok)
    }

    fn get_module_data<T: 'static>(&self, key: &str) -> Option<&T> {
        // It's technically unsafe to do this, but we assume that other modules
        // aren't going to go out of their way to find the key we've used
        // and corrupt its value's data.
        let full_key = module_data_key::<T>(key);
        let mut ptr: *const c_void = ptr::null();
        unsafe {
            ErrorCode::result_from(libpam_sys::pam_get_data(
                self.inner(),
                full_key.as_ptr(),
                &mut ptr,
            ))
            .ok()?;

            (ptr as *const T).as_ref()
        }
    }

    fn set_module_data<T: 'static>(&mut self, key: &str, data: T) -> Result<()> {
        let full_key = module_data_key::<T>(key);
        let data = Box::new(data);
        ErrorCode::result_from(unsafe {
            libpam_sys::pam_set_data(
                self.inner_mut(),
                full_key.as_ptr(),
                Box::into_raw(data).cast(),
                drop_module_data::<T>,
            )
        })
    }

    fn authtok_item(&self) -> Result<Option<OsString>> {
        unsafe { items::get_cstr_item(self, ItemType::AuthTok) }
    }
    fn old_authtok_item(&self) -> Result<Option<OsString>> {
        unsafe { items::get_cstr_item(self, ItemType::OldAuthTok) }
    }
}

/// Constructs a type-specific, module-specific key for this data.
fn module_data_key<T: 'static>(key: &str) -> CString {
    // The type ID is unique per-type.
    let tid = TypeId::of::<T>();
    // The `set_data_cleanup` function lives statically inside each PAM module,
    // so its address will be different between `pam_a.so` and `pam_b.so`,
    // even if both modules .so files are byte-for-byte identical.
    let cleanup_addr = drop_module_data::<T> as usize;
    // Then, by adding the key,
    let key = format!("{key:?}::{tid:?}::{cleanup_addr:016x}");
    CString::new(key).expect("null bytes somehow got into a debug string?")
}

/// Function called at the end of a PAM session that is called to clean up
/// a value previously provided to PAM in a `pam_set_data` call.
///
/// You should never call this yourself.
extern "C" fn drop_module_data<T>(_: *mut libpam_sys::pam_handle, c_data: *mut c_void, _: c_int) {
    unsafe {
        // Adopt the pointer into a Box and immediately drop it.
        let _: Box<T> = Box::from_raw(c_data.cast());
    }
}

// Implementations of internal functions.
impl LibPamHandle {
    #[cfg(any(pam_impl = "LinuxPam", pam_impl = "OpenPam"))]
    fn get_authtok(&mut self, prompt: Option<&OsStr>, item_type: ItemType) -> Result<OsString> {
        let prompt = memory::option_cstr_os(prompt);
        let mut output: *const c_char = ptr::null();
        // SAFETY: We're calling this with known-good values.
        let res = unsafe {
            libpam_sys::pam_get_authtok(
                self.inner_mut(),
                item_type.into(),
                &mut output,
                memory::prompt_ptr(prompt.as_deref()),
            )
        };
        ErrorCode::result_from(res)?;
        // SAFETY: We got this string from PAM.
        unsafe { memory::copy_pam_string(output) }.ok_or(ErrorCode::ConversationError)
    }

    #[cfg(pam_impl = "Sun")]
    fn get_authtok(&mut self, _prompt: Option<&OsStr>, item_type: ItemType) -> Result<OsString> {
        unsafe { items::get_cstr_item(self, item_type) }?.ok_or(ErrorCode::ConversationError)
    }

    #[cfg(not(any(pam_impl = "LinuxPam", pam_impl = "OpenPam", pam_impl = "Sun")))]
    fn get_authtok(&mut self, _: Option<&OsStr>, _: ItemType) -> Result<OsString> {
        // We don't have an authtok implementation for other PAM implementations.
        Err(ErrorCode::ConversationError)
    }

    /// Gets the `PAM_CONV` item from the handle.
    fn conversation_item(&self) -> Result<&PamConv> {
        let mut output: *const c_void = ptr::null();
        let result = unsafe {
            libpam_sys::pam_get_item(self.inner(), ItemType::Conversation.into(), &mut output)
        };
        ErrorCode::result_from(result)?;
        let output: *const PamConv = output.cast();
        // SAFETY: We got this result from PAM, and we're checking if it's null.
        unsafe { output.as_ref() }.ok_or(ErrorCode::ConversationError)
    }
}
