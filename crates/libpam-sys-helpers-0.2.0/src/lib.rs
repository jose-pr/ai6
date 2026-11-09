//! This package contains helpers to deal with memory management and
//! annoying type stuff in `libpam-sys` (and LibPAM in general).

use std::error::Error;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::ManuallyDrop;
use std::ptr::NonNull;
use std::{any, fmt, mem, ptr, slice};

// Memory management

/// A pointer-to-pointer-to-message container for PAM's conversation callback.
///
/// The PAM conversation callback requires a pointer to a pointer of
/// `pam_message`s. Linux-PAM handles this differently than all other
/// PAM implementations (including the X/SSO PAM standard).
///
/// X/SSO appears to specify a pointer-to-pointer-to-array:
///
/// ```text
///           points to  ┌────────────┐       ╔═ Message[] ═╗
/// messages ┄┄┄┄┄┄┄┄┄┄> │ *messages ┄┼┄┄┄┄┄> ║ style       ║
///                      └────────────┘       ║ data ┄┄┄┄┄┄┄╫┄┄> ...
///                                           ╟─────────────╢
///                                           ║ style       ║
///                                           ║ data ┄┄┄┄┄┄┄╫┄┄> ...
///                                           ╟─────────────╢
///                                           ║ ...         ║
/// ```
///
/// whereas Linux-PAM uses an `**argv`-style pointer-to-array-of-pointers:
///
/// ```text
///           points to  ┌──────────────┐      ╔═ Message ═╗
/// messages ┄┄┄┄┄┄┄┄┄┄> │ messages[0] ┄┼┄┄┄┄> ║ style     ║
///                      │ messages[1] ┄┼┄┄┄╮  ║ data ┄┄┄┄┄╫┄┄> ...
///                      │ ...          │   ┆  ╚═══════════╝
///                                         ┆
///                                         ┆    ╔═ Message ═╗
///                                         ╰┄┄> ║ style     ║
///                                              ║ data ┄┄┄┄┄╫┄┄> ...
///                                              ╚═══════════╝
/// ```
///
/// Because the `messages` remain owned by the application which calls into PAM,
/// we can solve this with One Simple Trick: make the intermediate list point
/// into the same array:
///
/// ```text
///           points to  ┌──────────────┐      ╔═ Message[] ═╗
/// messages ┄┄┄┄┄┄┄┄┄┄> │ messages[0] ┄┼┄┄┄┄> ║ style       ║
///                      │ messages[1] ┄┼┄┄╮   ║ data ┄┄┄┄┄┄┄╫┄┄> ...
///                      │ ...          │  ┆   ╟─────────────╢
///                                        ╰┄> ║ style       ║
///                                            ║ data ┄┄┄┄┄┄┄╫┄┄> ...
///                                            ╟─────────────╢
///                                            ║ ...         ║
/// ```
#[derive(Debug)]
pub struct PtrPtrVec<T> {
    data: Vec<T>,
    pointers: Vec<*const T>,
}

// Since this is a wrapper around a Vec with no dangerous functionality*,
// this can be Send and Sync provided the original Vec is.
//
// * It will only become unsafe when the user dereferences a pointer or sends it
// to an unsafe function.
unsafe impl<T> Send for PtrPtrVec<T> where Vec<T>: Send {}
unsafe impl<T> Sync for PtrPtrVec<T> where Vec<T>: Sync {}

impl<T> PtrPtrVec<T> {
    /// Takes ownership of the given Vec and creates a vec of pointers to it.
    pub fn new(data: Vec<T>) -> Self {
        let start = data.as_ptr();
        // We do this slightly tricky little dance to satisfy Miri:
        //
        // A pointer extracted from a reference can only legally access
        // that reference's memory. This means that if we say:
        //     pointers[0] = &data[0] as *const T;
        // we can't traverse through pointers[0] to reach data[1],
        // we can only use pointers[1].
        //
        // However, if we use the start-of-vec pointer from the `data` vector,
        // its "provenance"* is valid for the entire array (even if the address
        // of the pointer is the same). This avoids some behavior which is
        // technically undefined. While the CPU sees no difference between
        // those two pointers, the compiler is allowed to make optimizations
        // based on that provenance (even if, in this case, it isn't likely
        // to do so).
        //
        //       data.as_ptr() points here, and is valid for the whole Vec.
        //       ┃
        //       ┠─────────────────╮
        //       ┌─────┬─────┬─────┐
        //  data │ [0] │ [1] │ [2] │
        //       └─────┴─────┴─────┘
        //       ┠─────╯     ┊
        //       ┃     ┊     ┊
        //       (&data[0] as *const T) points to the same place, but is valid
        //       only for that 0th element.
        //             ┊     ┊
        //             ┠─────╯
        //             ┃
        //             (&data[1] as *const T) points here, and is only valid
        //             for that element.
        //
        // We only have to do this for pointers[0] because only that pointer
        // is used for accessing elements other than data[0] (in XSSO).
        //
        // * "provenance" is kind of like if every pointer in your program
        // remembered where it came from and, based on that, it had an implied
        // memory range it was valid for, separate from its address.
        // https://doc.rust-lang.org/std/ptr/#provenance
        // (It took a long time for me to understand this.)
        let mut pointers = Vec::with_capacity(data.len());
        // Ensure the 0th pointer has provenance from the entire vec
        // (even though it's numerically identical to &data[0] as *const T).
        pointers.push(start);
        // The 1st and everything thereafter only need to have the provenance
        // of their own memory.
        pointers.extend(data[1..].iter().map(|r| r as *const T));
        Self { data, pointers }
    }

    /// Gives you back your Vec.
    pub fn into_inner(self) -> Vec<T> {
        self.data
    }

    /// Gets a pointer-to-pointer suitable for passing into the Conversation.
    pub fn as_ptr<Dest>(&self) -> *const *const Dest {
        Self::assert_size::<Dest>();
        self.pointers.as_ptr().cast::<*const Dest>()
    }

    /// Iterates over a Linux-PAM–style pointer-to-array-of-pointers.
    ///
    /// # Safety
    ///
    /// `ptr_ptr` must be a valid pointer to an array of pointers,
    /// there must be at least `count` valid pointers in the array,
    /// and each pointer in that array must point to a valid `T`.
    #[deprecated = "use [`Self::iter_over`] instead, unless you really need this specific version"]
    #[allow(dead_code)]
    pub unsafe fn iter_over_linux<'a, Src>(
        ptr_ptr: *const *const Src,
        count: usize,
    ) -> impl Iterator<Item = &'a T>
    where
        T: 'a,
    {
        Self::assert_size::<Src>();
        slice::from_raw_parts(ptr_ptr.cast::<&T>(), count)
            .iter()
            .copied()
    }

    /// Iterates over an X/SSO–style pointer-to-pointer-to-array.
    ///
    /// # Safety
    ///
    /// You must pass a valid pointer to a valid pointer to an array,
    /// there must be at least `count` elements in the array,
    /// and each value in that array must be a valid `T`.
    #[deprecated = "use [`Self::iter_over`] instead, unless you really need this specific version"]
    #[allow(dead_code)]
    pub unsafe fn iter_over_xsso<'a, Src>(
        ptr_ptr: *const *const Src,
        count: usize,
    ) -> impl Iterator<Item = &'a T>
    where
        T: 'a,
    {
        Self::assert_size::<Src>();
        slice::from_raw_parts(*ptr_ptr.cast(), count).iter()
    }

    /// Iterates over a PAM message list appropriate to your system's impl.
    ///
    /// This selects the correct pointer/array structure to use for a message
    /// that was given to you by your system.
    ///
    /// # Safety
    ///
    /// `ptr_ptr` must point to a valid message list, there must be at least
    /// `count` messages in the list, and all messages must be a valid `Src`.
    #[allow(deprecated)]
    pub unsafe fn iter_over<'a, Src>(
        ptr_ptr: *const *const Src,
        count: usize,
    ) -> impl Iterator<Item = &'a T>
    where
        T: 'a,
    {
        #[cfg(pam_impl = "LinuxPam")]
        return Self::iter_over_linux(ptr_ptr, count);
        #[cfg(not(pam_impl = "LinuxPam"))]
        return Self::iter_over_xsso(ptr_ptr, count);
    }

    fn assert_size<That>() {
        assert_eq!(
            mem::size_of::<T>(),
            mem::size_of::<That>(),
            "type {t} is not the size of {that}",
            t = any::type_name::<T>(),
            that = any::type_name::<That>(),
        );
    }
}

/// Error returned when attempting to allocate a buffer that is too big.
///
/// This is specifically used in [`OwnedBinaryPayload`] when you try to allocate
/// a message larger than 2<sup>32</sup> bytes.
#[derive(Debug, PartialEq)]
pub struct TooBigError {
    pub size: usize,
    pub max: usize,
}

impl Error for TooBigError {}

impl fmt::Display for TooBigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "can't allocate a message of {size} bytes (max {max})",
            size = self.size,
            max = self.max
        )
    }
}

/// A trait wrapping memory management.
///
/// This is intended to allow you to bring your own allocator for
/// [`OwnedBinaryPayload`]s.
///
/// For an implementation example, see the implementation of this trait
/// for [`Vec`].
#[allow(clippy::wrong_self_convention)]
pub trait Buffer {
    /// Allocates a buffer of `len` elements, filled with the default.
    fn allocate(len: usize) -> Self;

    fn as_ptr(this: &Self) -> *const u8;

    /// Returns a slice view of `size` elements of the given memory.
    ///
    /// # Safety
    ///
    /// The caller must not request more elements than are allocated.
    unsafe fn as_mut_slice(this: &mut Self, len: usize) -> &mut [u8];

    /// Consumes this ownership and returns a pointer to the start of the arena.
    fn into_ptr(this: Self) -> NonNull<u8>;

    /// "Adopts" the memory at the given pointer, taking it under management.
    ///
    /// Running the operation:
    ///
    /// ```
    /// # use libpam_sys_helpers::Buffer;
    /// # fn test<T: Default, OwnerType: Buffer>(bytes: usize) {
    /// let owner = OwnerType::allocate(bytes);
    /// let ptr = OwnerType::into_ptr(owner);
    /// let owner = unsafe { OwnerType::from_ptr(ptr, bytes) };
    /// # }
    /// ```
    ///
    /// must be a no-op.
    ///
    /// # Safety
    ///
    /// The pointer must be valid, and the caller must provide the exact size
    /// of the given arena.
    unsafe fn from_ptr(ptr: NonNull<u8>, bytes: usize) -> Self;
}

impl Buffer for Vec<u8> {
    fn allocate(bytes: usize) -> Self {
        vec![0; bytes]
    }

    fn as_ptr(this: &Self) -> *const u8 {
        Vec::as_ptr(this)
    }

    unsafe fn as_mut_slice(this: &mut Self, bytes: usize) -> &mut [u8] {
        &mut this[..bytes]
    }

    fn into_ptr(this: Self) -> NonNull<u8> {
        let mut me = ManuallyDrop::new(this);
        // SAFETY: a Vec is guaranteed to have a nonzero pointer.
        unsafe { NonNull::new_unchecked(me.as_mut_ptr()) }
    }

    unsafe fn from_ptr(ptr: NonNull<u8>, bytes: usize) -> Self {
        Vec::from_raw_parts(ptr.as_ptr(), bytes, bytes)
    }
}

/// The structure of the "binary message" payload for the `PAM_BINARY_PROMPT`
/// extension from Linux-PAM.
pub struct BinaryPayload {
    /// The total byte size of the message, including this header,
    /// as u32 in network byte order (big endian).
    pub total_bytes_u32be: [u8; 4],
    /// A tag used to provide some kind of hint as to what the data is.
    /// Its meaning is undefined.
    pub data_type: u8,
    /// Where the data itself would start, used as a marker to make this
    /// not [`Unpin`] (since it is effectively an intrusive data structure
    /// pointing to immediately after itself).
    pub _marker: PhantomData<PhantomPinned>,
}

impl BinaryPayload {
    /// The most data it's possible to put into a [`BinaryPayload`].
    pub const MAX_SIZE: usize = (u32::MAX - 5) as usize;

    /// Fills in the provided buffer with the given data.
    ///
    /// This uses [`copy_from_slice`](slice::copy_from_slice) internally,
    /// so `buf` must be exactly 5 bytes longer than `data`, or this function
    /// will panic.
    pub fn fill(buf: &mut [u8], data: &[u8], data_type: u8) {
        let ptr: *mut Self = buf.as_mut_ptr().cast();
        // SAFETY: We're given a slice, which always has a nonzero pointer.
        let me = unsafe { ptr.as_mut().unwrap_unchecked() };
        me.total_bytes_u32be = u32::to_be_bytes(buf.len() as u32);
        me.data_type = data_type;
        buf[5..].copy_from_slice(data)
    }

    /// The total storage needed for the message, including header.
    ///
    /// # Safety
    ///
    /// The pointer must point to a valid `BinaryPayload`.
    pub unsafe fn total_bytes(this: *const Self) -> usize {
        let header = this.as_ref().unwrap_unchecked();
        u32::from_be_bytes(header.total_bytes_u32be) as usize
    }

    /// Gets the total byte buffer of the BinaryMessage stored at the pointer.
    ///
    /// The returned data slice is borrowed from where the pointer points to.
    ///
    /// # Safety
    ///
    /// - The pointer must point to a valid `BinaryPayload`.
    /// - The borrowed data must not outlive the pointer's validity.
    pub unsafe fn buffer_of<'a>(ptr: *const Self) -> &'a [u8] {
        slice::from_raw_parts(ptr.cast(), Self::total_bytes(ptr).max(5))
    }

    /// Gets the contents of the BinaryMessage stored at the given pointer.
    ///
    /// The returned data slice is borrowed from where the pointer points to.
    /// This is a cheap operation and doesn't do *any* copying.
    ///
    /// We don't take a `&self` reference here because accessing beyond
    /// the range of the `Self` data (i.e., beyond the 5 bytes of `self`)
    /// is undefined behavior. Instead, you have to pass a raw pointer
    /// directly to the data.
    ///
    /// # Safety
    ///
    /// - The pointer must point to a valid `BinaryPayload`.
    /// - The borrowed data must not outlive the pointer's validity.
    pub unsafe fn contents<'a>(ptr: *const Self) -> (&'a [u8], u8) {
        let header: &Self = ptr.as_ref().unwrap_unchecked();
        (&Self::buffer_of(ptr)[5..], header.data_type)
    }

    /// Zeroes out the data of this payload.
    ///
    /// # Safety
    ///
    /// - The pointer must point to a valid `BinaryPayload`.
    /// - The binary payload must not be used in the future,
    ///   since its length metadata is gone and so its buffer is unknown.
    pub unsafe fn zero(ptr: *mut Self) {
        let size = Self::total_bytes(ptr);
        let ptr: *mut u8 = ptr.cast();
        for x in 0..size {
            ptr::write_volatile(ptr.byte_add(x), mem::zeroed())
        }
    }
}

/// A binary message owned by some storage.
///
/// This is an owned, memory-managed version of [`BinaryPayload`].
/// The `O` type manages the memory where the payload lives.
/// [`Vec<u8>`] is one such manager and can be used when ownership
/// of the data does not need to transit through PAM.
#[derive(Debug)]
pub struct OwnedBinaryPayload<Owner: Buffer>(Owner);

impl<O: Buffer> OwnedBinaryPayload<O> {
    /// Allocates a new OwnedBinaryPayload.
    ///
    /// This will return a [`TooBigError`] if you try to allocate too much
    /// (more than [`BinaryPayload::MAX_SIZE`]).
    pub fn new(data: &[u8], type_: u8) -> Result<Self, TooBigError> {
        let total_len: u32 = (data.len() + 5).try_into().map_err(|_| TooBigError {
            size: data.len(),
            max: BinaryPayload::MAX_SIZE,
        })?;
        let total_len = total_len as usize;
        let mut buf = O::allocate(total_len);
        // SAFETY: We just allocated this exact size.
        BinaryPayload::fill(
            unsafe { Buffer::as_mut_slice(&mut buf, total_len) },
            data,
            type_,
        );
        Ok(Self(buf))
    }

    /// The contents of the buffer.
    pub fn contents(&self) -> (&[u8], u8) {
        unsafe { BinaryPayload::contents(self.as_ptr()) }
    }

    /// The total bytes needed to store this, including the header.
    pub fn total_bytes(&self) -> usize {
        unsafe { BinaryPayload::buffer_of(Buffer::as_ptr(&self.0).cast()).len() }
    }

    /// Unwraps this into the raw storage backing it.
    pub fn into_inner(self) -> O {
        self.0
    }

    /// Gets a const pointer to the start of the message's buffer.
    pub fn as_ptr(&self) -> *const BinaryPayload {
        Buffer::as_ptr(&self.0).cast()
    }

    /// Consumes ownership of this message and converts it to a raw pointer
    /// to the start of the message.
    ///
    /// To clean this up, you should eventually pass it into [`Self::from_ptr`]
    /// with the same `O` ownership type.
    pub fn into_ptr(self) -> NonNull<BinaryPayload> {
        Buffer::into_ptr(self.0).cast()
    }

    /// Takes ownership of the given pointer.
    ///
    /// # Safety
    ///
    /// You must provide a valid pointer, allocated by (or equivalent to one
    /// allocated by) [`Self::new`]. For instance, passing a pointer allocated
    /// by `malloc` to `OwnedBinaryPayload::<Vec<u8>>::from_ptr` is not allowed.
    pub unsafe fn from_ptr(ptr: NonNull<BinaryPayload>) -> Self {
        Self(O::from_ptr(
            ptr.cast(),
            BinaryPayload::total_bytes(ptr.as_ptr()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    type VecPayload = OwnedBinaryPayload<Vec<u8>>;

    #[test]
    fn test_binary_payload() {
        let simple_message = &[0u8, 0, 0, 16, 0xff, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let empty = &[0u8; 5];

        assert_eq!((&[0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..], 0xff), unsafe {
            BinaryPayload::contents(simple_message.as_ptr().cast())
        });
        assert_eq!((&[][..], 0x00), unsafe {
            BinaryPayload::contents(empty.as_ptr().cast())
        });
    }

    #[test]
    fn test_owned_binary_payload() {
        let (data, typ) = (
            &[0, 1, 1, 8, 9, 9, 9, 8, 8, 1, 9, 9, 9, 1, 1, 9, 7, 2, 5, 3][..],
            112,
        );
        let payload = VecPayload::new(data, typ).unwrap();
        assert_eq!((data, typ), payload.contents());
        let ptr = payload.into_ptr();
        let payload = unsafe { VecPayload::from_ptr(ptr) };
        assert_eq!((data, typ), payload.contents());
    }

    #[test]
    #[ignore]
    fn test_owned_too_big() {
        let data = vec![0xFFu8; 0x1_0000_0001];
        assert_eq!(
            TooBigError {
                max: 0xffff_fffa,
                size: 0x1_0000_0001
            },
            VecPayload::new(&data, 5).unwrap_err()
        )
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic]
    fn test_new_wrong_size() {
        let bad_vec = vec![0; 19];
        let msg = PtrPtrVec::new(bad_vec);
        let _ = msg.as_ptr::<u64>();
    }

    #[allow(deprecated)]
    #[test]
    #[should_panic]
    fn test_iter_xsso_wrong_size() {
        unsafe {
            let _ = PtrPtrVec::<u8>::iter_over_xsso::<f64>(ptr::null(), 1);
        }
    }

    #[allow(deprecated)]
    #[test]
    #[should_panic]
    fn test_iter_linux_wrong_size() {
        unsafe {
            let _ = PtrPtrVec::<u128>::iter_over_linux::<()>(ptr::null(), 1);
        }
    }

    #[allow(deprecated)]
    #[test]
    fn test_right_size() {
        let good_vec = vec![(1u64, 2u64), (3, 4), (5, 6)];
        let ptr = good_vec.as_ptr();
        let msg = PtrPtrVec::new(good_vec);
        let msg_ref: *const *const (i64, i64) = msg.as_ptr();
        assert_eq!(unsafe { *msg_ref }, ptr.cast());

        let linux_result: Vec<(i64, i64)> = unsafe { PtrPtrVec::iter_over_linux(msg_ref, 3) }
            .cloned()
            .collect();
        let xsso_result: Vec<(i64, i64)> = unsafe { PtrPtrVec::iter_over_xsso(msg_ref, 3) }
            .cloned()
            .collect();
        assert_eq!(vec![(1, 2), (3, 4), (5, 6)], linux_result);
        assert_eq!(vec![(1, 2), (3, 4), (5, 6)], xsso_result);
        drop(msg)
    }

    #[allow(deprecated)]
    #[test]
    fn test_iter_ptr_ptr() {
        // These boxes are larger than a single pointer because we want to
        // make sure they're not accidentally allocated adjacently
        // in such a way that it's compatible with X/SSO.
        //
        // a pointer to (&str, i32) can be treated as a pointer to (&str).
        #[repr(C)]
        struct Pair(&'static str, i32);
        let boxes = vec![
            Box::new(Pair("a", 1)),
            Box::new(Pair("b", 2)),
            Box::new(Pair("c", 3)),
            Box::new(Pair("D", 4)),
        ];
        let ptr: *const *const &str = boxes.as_ptr().cast();
        let got: Vec<&str> = unsafe { PtrPtrVec::iter_over_linux(ptr, 4) }
            .cloned()
            .collect();
        assert_eq!(vec!["a", "b", "c", "D"], got);

        // On the other hand, we explicitly want these to be adjacent.
        let nums = [-1i8, 2, 3];
        let ptr = nums.as_ptr();
        let got: Vec<u8> = unsafe { PtrPtrVec::iter_over_xsso(&ptr, 3) }
            .cloned()
            .collect();
        assert_eq!(vec![255, 2, 3], got);
    }
}
