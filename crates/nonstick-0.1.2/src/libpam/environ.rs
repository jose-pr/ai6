use crate::environ::{EnvironMap, EnvironMapMut};
use crate::libpam::memory::{CHeapBox, CHeapString};
use crate::libpam::{memory, LibPamHandle};
use std::ffi::{c_char, CStr, CString, OsStr, OsString};
use std::marker::PhantomData;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::ptr;
use std::ptr::NonNull;

impl LibPamHandle {
    fn environ_get(&self, key: &OsStr) -> Option<OsString> {
        let key = CString::new(key.as_bytes()).ok()?;
        // SAFETY: We are a valid handle and are calling with a good key.
        let src = unsafe { libpam_sys::pam_getenv(self.inner(), key.as_ptr()) };
        let val = match NonNull::new(src) {
            None => return None,
            Some(ptr) => ptr.as_ptr(),
        };
        // SAFETY: We were just returned this string from PAM.
        // We have to trust it.
        let c_str = unsafe { CStr::from_ptr(val) };
        Some(OsString::from_vec(c_str.to_bytes().to_vec()))
    }

    fn environ_set(&mut self, key: &OsStr, value: Option<&OsStr>) -> Option<OsString> {
        let old = self.environ_get(key);
        if old.is_none() && value.is_none() {
            // pam_putenv returns an error if we try to remove a non-existent
            // environment variable, so just avoid that entirely.
            return None;
        }
        let total_len = key.len() + value.map(OsStr::len).unwrap_or_default() + 2;
        let mut result = Vec::with_capacity(total_len);
        result.extend(key.as_bytes());
        if let Some(value) = value {
            result.push(b'=');
            result.extend(value.as_bytes());
        }
        let put = CString::new(result).unwrap();
        // SAFETY: This is a valid handle and a valid environment string.
        // pam_putenv is only ever going to
        let _ = unsafe { libpam_sys::pam_putenv(self.inner_mut(), put.as_ptr()) };
        old
    }

    fn environ_iter(&self) -> impl Iterator<Item = (OsString, OsString)> {
        // SAFETY: This is a valid PAM handle. It will return valid data.
        unsafe {
            NonNull::new(libpam_sys::pam_getenvlist(self.inner()))
                .map(|ptr| EnvList::from_ptr(ptr.cast()))
                .unwrap_or_else(EnvList::empty)
        }
    }
}

/// A view to the environment stored in a PAM handle.
pub struct LibPamEnviron<'a> {
    source: &'a LibPamHandle,
}

/// A mutable view to the environment stored in a PAM handle.
pub struct LibPamEnvironMut<'a> {
    source: &'a mut LibPamHandle,
}

impl<'a> LibPamEnviron<'a> {
    pub fn new(source: &'a LibPamHandle) -> Self {
        Self { source }
    }
}

impl<'a> LibPamEnvironMut<'a> {
    pub fn new(source: &'a mut LibPamHandle) -> Self {
        Self { source }
    }
}

impl EnvironMap<'_> for LibPamEnviron<'_> {
    fn get(&self, key: impl AsRef<OsStr>) -> Option<OsString> {
        self.source.environ_get(key.as_ref())
    }

    fn iter(&self) -> impl Iterator<Item = (OsString, OsString)> {
        self.source.environ_iter()
    }
}

impl EnvironMap<'_> for LibPamEnvironMut<'_> {
    fn get(&self, key: impl AsRef<OsStr>) -> Option<OsString> {
        self.source.environ_get(key.as_ref())
    }

    fn iter(&self) -> impl Iterator<Item = (OsString, OsString)> {
        self.source.environ_iter()
    }
}

impl EnvironMapMut<'_> for LibPamEnvironMut<'_> {
    fn insert(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> Option<OsString> {
        self.source.environ_set(key.as_ref(), Some(val.as_ref()))
    }

    fn remove(&mut self, key: impl AsRef<OsStr>) -> Option<OsString> {
        self.source.environ_set(key.as_ref(), None)
    }
}

struct EnvList<'a> {
    /// Pointer to the start of the environment variable list.
    ///
    /// This can't be a `CHeapBox` because it's not just a single
    /// `Option<EnvVar>`.
    start: NonNull<Option<EnvVar>>,
    /// The environment variable we're about to iterate into.
    current: NonNull<Option<EnvVar>>,
    _owner: PhantomData<&'a LibPamHandle>,
}

impl EnvList<'_> {
    fn empty() -> Self {
        let none: CHeapBox<Option<EnvVar>> = CHeapBox::new(None);
        let ptr = CHeapBox::into_ptr(none);
        Self {
            start: ptr,
            current: ptr,
            _owner: PhantomData,
        }
    }
    unsafe fn from_ptr(ptr: NonNull<*mut c_char>) -> Self {
        Self {
            start: ptr.cast(),
            current: ptr.cast(),
            _owner: Default::default(),
        }
    }
}

impl Iterator for EnvList<'_> {
    type Item = (OsString, OsString);

    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY: We were given a pointer to a valid environment list,
        // and we only ever advance it to the exact end of the list.
        match unsafe { self.current.as_mut() } {
            None => None,
            Some(item) => {
                let ret = item.as_kv();
                // SAFETY: We know we're still pointing to a valid pointer,
                // and advancing it one more is allowed.
                unsafe {
                    self.current = advance(self.current);
                    ptr::drop_in_place(item as *mut EnvVar);
                }
                Some(ret)
            }
        }
    }
}

impl Drop for EnvList<'_> {
    fn drop(&mut self) {
        // SAFETY: We own self.start, and we know that self.current points to
        // either an item we haven't used, or to the None end.
        unsafe {
            while let Some(var_ref) = self.current.as_mut() {
                self.current = advance(self.current);
                ptr::drop_in_place(var_ref as *mut EnvVar);
            }
            memory::free(self.start.as_ptr())
        }
    }
}

unsafe fn advance<T>(nn: NonNull<T>) -> NonNull<T> {
    NonNull::new_unchecked(nn.as_ptr().offset(1))
}

struct EnvVar(CHeapString);

impl EnvVar {
    fn as_kv(&self) -> (OsString, OsString) {
        let bytes = self.0.to_bytes();
        let mut split = bytes.splitn(2, |&b| b == b'=');
        (
            OsString::from_vec(split.next().unwrap_or_default().into()),
            OsString::from_vec(split.next().unwrap_or_default().into()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(text: &str) -> OsString {
        OsString::from_vec(text.into())
    }

    #[test]
    fn test_split_kv() {
        fn test(input: &str, key: &str, value: &str) {
            let data = CHeapString::new(input);
            let key = os(key);
            let value = os(value);

            assert_eq!(EnvVar(data).as_kv(), (key, value));
        }
        test("THIS=that", "THIS", "that");
        test("THESE=those, no one=knows", "THESE", "those, no one=knows");
        test("HERE=", "HERE", "");
        test("SOME", "SOME", "");
        test("", "", "");
    }

    fn env_list(strings: &[&'static str]) -> EnvList<'static> {
        let ptrs: NonNull<Option<CHeapString>> = memory::calloc(strings.len() + 1);
        unsafe {
            for (idx, &text) in strings.iter().enumerate() {
                ptr::write(ptrs.as_ptr().add(idx), Some(CHeapString::new(text)))
            }
            ptr::write(ptrs.as_ptr().add(strings.len()), None);
            EnvList::from_ptr(ptrs.cast())
        }
    }

    #[test]
    fn test_iter() {
        let envs = env_list(&["ONE=two", "BIRDS=birds=birds", "me", "you="]);
        let result: Vec<_> = envs.collect();
        assert_eq!(
            vec![
                (os("ONE"), os("two")),
                (os("BIRDS"), os("birds=birds")),
                (os("me"), os("")),
                (os("you"), os("")),
            ],
            result
        );
    }

    #[test]
    fn test_iter_partial() {
        let mut envs = env_list(&[
            "iterating=this",
            "also=here",
            "but not=this one",
            "or even=the last",
        ]);

        assert_eq!(Some((os("iterating"), os("this"))), envs.next());
        assert_eq!(Some((os("also"), os("here"))), envs.next());
        // let envs drop
    }
}
