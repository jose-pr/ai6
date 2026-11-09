//! Things for dealing with memory.

use libpam_sys_helpers::{Buffer, OwnedBinaryPayload};
use std::ffi::{c_char, CStr, CString, OsStr, OsString};
use std::marker::{PhantomData, PhantomPinned};
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::ptr::NonNull;
use std::{mem, ptr, slice};

/// Like the num_enum crate, but with no dependency on `syn`.
macro_rules! num_enum {
    (
        $(#[$m:meta])*
        $viz:vis enum $name:ident {
            $(
                $(#[$im:meta])*
                $item_name:ident = $item_value:path,
            )*
        }
    ) => {
        // This is the one place where we depend upon c_int being i32.
        // Ideally, we would be able to say `repr(c_int)` but we can't.
        $(#[$m])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
        #[repr(i32)]
        $viz enum $name {
            $(
                $(#[$im])*
                $item_name = $item_value,
            )*
        }

        impl TryFrom<c_int> for $name {
            type Error = crate::constants::ErrorCode;

            #[allow(unused_doc_comments)]
            fn try_from(value: c_int) -> crate::constants::Result<$name> {
                match value {
                    $(
                        $(#[$im])*
                        $item_value => Ok(Self::$item_name),
                    )*
                    _ => Err(crate::constants::ErrorCode::BAD_CONST),
                }
            }
        }

        impl From<$name> for c_int {
            fn from(value: $name) -> c_int {
                value as c_int
            }
        }
    }
}

pub(crate) use num_enum;

/// Allocates `count` elements to hold `T`.
#[inline]
pub fn calloc<T>(count: usize) -> NonNull<T> {
    // SAFETY: it's always safe to allocate! Leaking memory is fun!
    unsafe { NonNull::new_unchecked(libc::calloc(count, mem::size_of::<T>()).cast()) }
}

/// Wrapper for [`libc::free`] to make debugging calls/frees easier.
///
/// # Safety
///
/// If you double-free, it's all your fault.
#[inline]
pub unsafe fn free<T>(p: *mut T) {
    libc::free(p.cast())
}

/// Makes whatever it's in not [`Send`], [`Sync`], or [`Unpin`].
#[repr(C)]
#[derive(Debug, Default)]
pub struct Immovable(pub PhantomData<(*mut u8, PhantomPinned)>);

/// Safely converts a `&str` option to a `CString` option.
pub fn option_cstr(prompt: Option<&[u8]>) -> Option<CString> {
    prompt.map(|p| CString::new(p).expect("nul is not allowed"))
}

pub fn option_cstr_os(prompt: Option<&OsStr>) -> Option<CString> {
    option_cstr(prompt.map(OsStr::as_bytes))
}

/// Gets the pointer to the given CString, or a null pointer if absent.
pub fn prompt_ptr(prompt: Option<&CStr>) -> *const c_char {
    match prompt {
        Some(c_str) => c_str.as_ptr(),
        None => ptr::null(),
    }
}

/// It's like a [`Box`], but C heap managed.
#[derive(Debug)]
#[repr(transparent)]
pub struct CHeapBox<T>(NonNull<T>);

// Lots of "as" and "into" associated functions.
#[allow(clippy::wrong_self_convention)]
impl<T> CHeapBox<T> {
    /// Creates a new CHeapBox holding the given data.
    pub fn new(value: T) -> Self {
        let memory = calloc(1);
        unsafe { ptr::write(memory.as_ptr(), value) }
        // SAFETY: We literally just allocated this.
        Self(memory)
    }

    /// Takes ownership of the given pointer.
    ///
    /// # Safety
    ///
    /// You have to provide a valid pointer to the start of an allocation
    /// that was made with `malloc`.
    pub unsafe fn from_ptr(ptr: NonNull<T>) -> Self {
        Self(ptr)
    }

    /// Converts this CBox into a raw pointer.
    pub fn into_ptr(this: Self) -> NonNull<T> {
        ManuallyDrop::new(this).0
    }

    /// Gets a pointer from this but doesn't convert this into a raw pointer.
    ///
    /// You are responsible for ensuring the CHeapBox lives long enough.
    pub fn as_ptr(this: &Self) -> NonNull<T> {
        this.0
    }

    /// Because it's annoying to type `CHeapBox.as_ptr(...).as_ptr()`.
    pub fn as_raw_ptr(this: &Self) -> *mut T {
        this.0.as_ptr()
    }

    /// Converts this into a Box of a different type.
    ///
    /// # Safety
    ///
    /// The other type has to have the same size and alignment and
    /// have compatible drop behavior with respect to other resources.
    pub unsafe fn cast<R>(this: Self) -> CHeapBox<R> {
        mem::transmute(this)
    }
}

impl<T: Default> Default for CHeapBox<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl Buffer for CHeapBox<u8> {
    fn allocate(len: usize) -> Self {
        // SAFETY: This is all freshly-allocated memory!
        unsafe { Self::from_ptr(calloc(len)) }
    }

    fn as_ptr(this: &Self) -> *const u8 {
        this.0.as_ptr()
    }

    unsafe fn as_mut_slice(this: &mut Self, len: usize) -> &mut [u8] {
        slice::from_raw_parts_mut(this.0.as_ptr(), len)
    }

    fn into_ptr(this: Self) -> NonNull<u8> {
        CHeapBox::into_ptr(this)
    }

    unsafe fn from_ptr(ptr: NonNull<u8>, _: usize) -> Self {
        CHeapBox::from_ptr(ptr)
    }
}

pub type CHeapPayload = OwnedBinaryPayload<CHeapBox<u8>>;

impl<T> Deref for CHeapBox<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        // SAFETY: We own this pointer and it is guaranteed valid.
        unsafe { Self::as_ptr(self).as_ref() }
    }
}

impl<T> DerefMut for CHeapBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: We own this pointer and it is guaranteed valid.
        unsafe { Self::as_ptr(self).as_mut() }
    }
}

impl<T> Drop for CHeapBox<T> {
    fn drop(&mut self) {
        // SAFETY: We own a valid pointer, and will never use it after this.
        unsafe {
            let ptr = self.0.as_ptr();
            ptr::drop_in_place(ptr);
            free(ptr)
        }
    }
}

/// A null-terminated string allocated on the C heap.
///
/// Basically [`CString`], but managed by malloc.
#[derive(Debug)]
#[repr(transparent)]
pub struct CHeapString(CHeapBox<c_char>);

impl CHeapString {
    /// Creates a new C heap string with the given contents.
    pub fn new(text: impl AsRef<[u8]>) -> Self {
        let data = text.as_ref();
        if data.contains(&0) {
            panic!("you're not allowed to create a cstring with a nul inside!");
        }
        // +1 for the null terminator
        let data_alloc: NonNull<c_char> = calloc(data.len() + 1);
        // SAFETY: we just allocated this and we have enough room.
        unsafe {
            let dest = slice::from_raw_parts_mut(data_alloc.as_ptr().cast(), data.len());
            dest.copy_from_slice(data);
            Self(CHeapBox::from_ptr(data_alloc))
        }
    }

    /// Converts this C heap string into a raw pointer.
    ///
    /// You are responsible for freeing it later.
    pub fn into_ptr(self) -> NonNull<c_char> {
        let this = ManuallyDrop::new(self);
        CHeapBox::as_ptr(&this.0)
    }

    /// Converts this into a dumb box. It will no longer be zeroed upon drop.
    pub fn into_box(self) -> CHeapBox<c_char> {
        unsafe { mem::transmute(self) }
    }

    /// Takes ownership of a C heap string.
    ///
    /// # Safety
    ///
    /// You have to provide a pointer to the start of an allocation that is
    /// a valid 0-terminated C string.
    pub unsafe fn from_ptr(ptr: *mut c_char) -> Option<Self> {
        NonNull::new(ptr).map(|p| unsafe { Self(CHeapBox::from_ptr(p)) })
    }

    /// Takes ownership of a CHeapBox.
    ///
    /// # Safety
    ///
    /// The box has to point to a valid 0-terminated C string.
    pub unsafe fn from_box<T>(bx: CHeapBox<T>) -> Self {
        Self(CHeapBox::cast(bx))
    }

    /// Zeroes the contents of a C string.
    ///
    /// # Safety
    ///
    /// You have to provide a valid pointer to a null-terminated C string.
    pub unsafe fn zero(ptr: NonNull<c_char>) {
        let cstr = ptr.as_ptr();
        let len = libc::strlen(cstr.cast());
        for x in 0..len {
            ptr::write_volatile(cstr.byte_offset(x as isize), mem::zeroed())
        }
    }
}

impl Drop for CHeapString {
    fn drop(&mut self) {
        // SAFETY: We own a valid C String
        unsafe { Self::zero(CHeapBox::as_ptr(&self.0)) }
    }
}

impl Deref for CHeapString {
    type Target = CStr;

    fn deref(&self) -> &Self::Target {
        // SAFETY: We know we own a valid C string pointer.
        let ptr = CHeapBox::as_ptr(&self.0).as_ptr();
        unsafe { CStr::from_ptr(ptr) }
    }
}

/// Creates an owned copy of a string that is returned from a
/// <code>pam_get_<var>whatever</var></code> function.
///
/// # Safety
///
/// It's on you to provide a valid string.
pub unsafe fn copy_pam_string(result_ptr: *const c_char) -> Option<OsString> {
    NonNull::new(result_ptr.cast_mut())
        .map(NonNull::as_ptr)
        .map(|p| CStr::from_ptr(p))
        .map(CStr::to_bytes)
        .map(Vec::from)
        .map(OsString::from_vec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn test_box() {
        let drop_count: Cell<u32> = Cell::new(0);

        struct Dropper<'a>(&'a Cell<u32>);

        impl Drop for Dropper<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1)
            }
        }

        let mut dropbox = CHeapBox::new(Dropper(&drop_count));
        _ = dropbox;
        // ensure the old value is dropped when the new one is assigned.
        dropbox = CHeapBox::new(Dropper(&drop_count));
        assert_eq!(1, drop_count.get());
        *dropbox = Dropper(&drop_count);
        assert_eq!(2, drop_count.get());
        drop(dropbox);
        assert_eq!(3, drop_count.get());
    }

    #[test]
    fn test_strings() {
        let str = CHeapString::new("hello there");
        let str_ptr = str.into_ptr().as_ptr();
        unsafe {
            let copied = copy_pam_string(str_ptr).unwrap();
            assert_eq!("hello there", copied);
            CHeapString::zero(NonNull::new(str_ptr).unwrap());
            let idx_three = str_ptr.add(3).as_mut().unwrap();
            *idx_three = 0x80u8 as c_char;
            let zeroed = copy_pam_string(str_ptr).unwrap();
            assert!(zeroed.is_empty());
            let _ = CHeapString::from_ptr(str_ptr);
        }
    }

    #[test]
    #[should_panic]
    fn test_nul_string() {
        CHeapString::new("hell\0 there");
    }

    #[test]
    fn test_option_str() {
        let good = option_cstr(Some("whatever".as_ref()));
        assert_eq!("whatever", good.unwrap().to_str().unwrap());
        let no_str = option_cstr(None);
        assert!(no_str.is_none());
    }
    #[test]
    #[should_panic]
    fn test_nul_cstr() {
        option_cstr(Some("what\0ever".as_ref()));
    }

    #[test]
    fn test_prompt() {
        let prompt_cstr = CString::new("good").ok();
        let prompt = prompt_ptr(prompt_cstr.as_deref());
        assert!(!prompt.is_null());
        let no_prompt = prompt_ptr(None);
        assert!(no_prompt.is_null());
    }
}
