//! Types used to communicate data from the application to the module.

use crate::libpam::conversation::OwnedExchange;
use crate::libpam::memory;
use crate::libpam::memory::{CHeapBox, CHeapPayload, CHeapString, Immovable};
use crate::{ErrorCode, Result};
use libpam_sys_helpers::BinaryPayload;
use std::ffi::{c_int, c_void, CStr, OsStr};
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::os::unix::ffi::OsStrExt;
use std::ptr::NonNull;
use std::{iter, ptr, slice};

/// The corridor via which the answer to Messages navigate through PAM.
#[derive(Debug)]
pub struct Answers {
    /// The actual list of answers. This can't be a [`CHeapBox`] because
    /// this is the pointer to the start of an array, not a single Answer.
    base: NonNull<Answer>,
    count: usize,
}

impl Answers {
    /// Builds an Answers out of the given answered Message Q&As.
    pub fn build(value: Vec<OwnedExchange>) -> Result<Self> {
        let mut outputs = Self {
            base: memory::calloc(value.len()),
            count: value.len(),
        };
        // Even if we fail during this process, we still end up freeing
        // all allocated answer memory.
        for (input, output) in iter::zip(value, outputs.iter_mut()) {
            match input {
                OwnedExchange::MaskedPrompt(p) => TextAnswer::fill(output, &p.answer()?)?,
                OwnedExchange::Prompt(p) => TextAnswer::fill(output, &p.answer()?)?,
                OwnedExchange::Error(p) => {
                    TextAnswer::fill(output, p.answer().map(|_| "".as_ref())?)?
                }
                OwnedExchange::Info(p) => {
                    TextAnswer::fill(output, p.answer().map(|_| "".as_ref())?)?
                }
                // If we're here, that means that we *got* a Linux-PAM
                // question from PAM, so we're OK to answer it.
                OwnedExchange::RadioPrompt(p) => TextAnswer::fill(output, &(p.answer()?))?,
                OwnedExchange::BinaryPrompt(p) => {
                    BinaryAnswer::fill(output, (&p.answer()?).into())?
                }
            }
        }
        Ok(outputs)
    }

    /// Converts this into a `*Answer` for passing to PAM.
    ///
    /// This object is consumed and the `Answer` pointer now owns its data.
    /// It can be recreated with [`Self::from_c_heap`].
    pub fn into_ptr(self) -> *mut libpam_sys::pam_response {
        ManuallyDrop::new(self).base.as_ptr().cast()
    }

    /// Takes ownership of a list of answers allocated on the C heap.
    ///
    /// # Safety
    ///
    /// It's up to you to make sure you pass a valid pointer,
    /// like one that you got from PAM, or maybe [`Self::into_ptr`].
    pub unsafe fn from_c_heap(base: NonNull<libpam_sys::pam_response>, count: usize) -> Self {
        Answers {
            base: NonNull::new_unchecked(base.as_ptr().cast()),
            count,
        }
    }
}

impl Deref for Answers {
    type Target = [Answer];
    fn deref(&self) -> &Self::Target {
        // SAFETY: This is the memory we manage ourselves.
        unsafe { slice::from_raw_parts(self.base.as_ptr(), self.count) }
    }
}

impl DerefMut for Answers {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: This is the memory we manage ourselves.
        unsafe { slice::from_raw_parts_mut(self.base.as_ptr(), self.count) }
    }
}

impl Drop for Answers {
    fn drop(&mut self) {
        // SAFETY: We allocated this ourselves, or it was provided to us by PAM.
        // We own these pointers, and they will never be used after this.
        unsafe {
            for answer in self.iter_mut() {
                ptr::drop_in_place(answer)
            }
            memory::free(self.base.as_ptr())
        }
    }
}

/// Generic version of answer data.
///
/// This has the same structure as [`BinaryAnswer`] and [`TextAnswer`].
#[repr(C)]
#[derive(Debug, Default)]
pub struct Answer {
    /// Owned pointer to the data returned in an answer.
    /// For most answers, this will be a
    /// [`CHeapString`], but for
    /// [`BinaryQAndA`](crate::conv::BinaryQAndA)s (a Linux-PAM extension),
    /// this will be a [`CHeapBox`] of [`BinaryPayload`].
    pub data: Option<CHeapBox<c_void>>,
    /// Unused. Just here for the padding.
    return_code: c_int,
    _marker: Immovable,
}

#[repr(transparent)]
#[derive(Debug)]
pub struct TextAnswer(Answer);

impl TextAnswer {
    /// Interprets the provided `Answer` as a text answer.
    ///
    /// # Safety
    ///
    /// It's up to you to provide an answer that is a `TextAnswer`.
    pub unsafe fn upcast(from: &mut Answer) -> &mut Self {
        // SAFETY: We're provided a valid reference.
        &mut *(from as *mut Answer).cast::<Self>()
    }

    /// Converts the `Answer` to a `TextAnswer` with the given text.
    fn fill(dest: &mut Answer, text: &OsStr) -> Result<()> {
        let allocated = CHeapString::new(text.as_bytes());
        let _ = dest
            .data
            .replace(unsafe { CHeapBox::cast(allocated.into_box()) });
        Ok(())
    }

    /// Gets the string stored in this answer.
    pub fn contents(&self) -> Result<&str> {
        match self.0.data.as_ref() {
            None => Ok(""),
            Some(data) => {
                // SAFETY: This data is either passed from PAM (so we are forced
                // to trust it) or was created by us in TextAnswerInner::alloc.
                // In either case, it's going to be a valid null-terminated string.
                unsafe { CStr::from_ptr(CHeapBox::as_ptr(data).as_ptr().cast()) }
                    .to_str()
                    .map_err(|_| ErrorCode::ConversationError)
            }
        }
    }

    /// Zeroes out the answer data, frees it, and points our data to `null`.
    ///
    /// When this `TextAnswer` is part of an [`Answers`],
    /// this is optional (since that will perform the `free`),
    /// but it will clear potentially sensitive data.
    pub fn zero_contents(&mut self) {
        // SAFETY: We own this data and know it's valid.
        // If it's null, this is a no-op.
        // After we're done, it will be null.
        unsafe {
            if let Some(ptr) = self.0.data.as_ref() {
                CHeapString::zero(CHeapBox::as_ptr(ptr).cast());
            }
        }
    }
}

/// A [`Answer`] with [`BinaryPayload`] in it.
#[repr(transparent)]
#[derive(Debug)]
pub struct BinaryAnswer(Answer);

impl BinaryAnswer {
    /// Interprets the provided [`Answer`] as a binary answer.
    ///
    /// # Safety
    ///
    /// It's up to you to provide an answer that is a `BinaryAnswer`.
    pub unsafe fn upcast(from: &mut Answer) -> &mut Self {
        // SAFETY: We're provided a valid reference.
        &mut *(from as *mut Answer).cast::<Self>()
    }

    /// Fills in a [`Answer`] with the provided binary data.
    ///
    /// The `data_type` is a tag you can use for whatever.
    /// It is passed through PAM unchanged.
    ///
    /// The referenced data is copied to the C heap.
    /// We do not take ownership of the original data.
    pub fn fill(dest: &mut Answer, (data, type_): (&[u8], u8)) -> Result<()> {
        let payload = CHeapPayload::new(data, type_).map_err(|_| ErrorCode::BufferError)?;
        let _ = dest
            .data
            .replace(unsafe { CHeapBox::cast(payload.into_inner()) });
        Ok(())
    }

    /// Gets the binary data in this answer.
    pub fn contents(&self) -> Option<(&[u8], u8)> {
        // SAFETY: We either got this data from PAM or allocated it ourselves.
        // Either way, we trust that it is either valid data or null.
        self.0
            .data
            .as_ref()
            .map(|data| unsafe { BinaryPayload::contents(CHeapBox::as_ptr(data).cast().as_ptr()) })
    }

    /// Zeroes out the answer data, frees it, and points our data to `null`.
    ///
    /// When this `BinaryAnswer` is part of an [`Answers`],
    /// this is optional (since that will perform the `free`),
    /// but it will clear potentially sensitive data.
    pub fn zero_contents(&mut self) {
        // SAFETY: We know that our data pointer is either valid or null.
        if let Some(data) = self.0.data.as_mut() {
            unsafe {
                let total = BinaryPayload::total_bytes(CHeapBox::as_ptr(data).cast().as_ref());
                let data: &mut [u8] =
                    slice::from_raw_parts_mut(CHeapBox::as_raw_ptr(data).cast(), total);
                data.fill(0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conv::{ErrorMsg, InfoMsg, MaskedQAndA, QAndA};

    macro_rules! answered {
        ($typ:ty, $msg:path, $data:expr) => {{
            let qa = <$typ>::new("".as_ref());
            qa.set_answer(Ok($data));
            $msg(qa)
        }};
    }

    fn assert_text_answer(want: &str, answer: &mut Answer) {
        let up = unsafe { TextAnswer::upcast(answer) };
        assert_eq!(want, up.contents().unwrap());
        up.zero_contents();
        assert_eq!("", up.contents().unwrap());
    }

    fn round_trip(exchanges: Vec<OwnedExchange>) -> Answers {
        let n = exchanges.len();
        let sent = Answers::build(exchanges).unwrap();
        unsafe { Answers::from_c_heap(NonNull::new_unchecked(sent.into_ptr()), n) }
    }

    #[test]
    fn test_round_trip() {
        let mut answers = round_trip(vec![
            answered!(QAndA, OwnedExchange::Prompt, "whats going on".into()),
            answered!(MaskedQAndA, OwnedExchange::MaskedPrompt, "well then".into()),
            answered!(ErrorMsg, OwnedExchange::Error, ()),
            answered!(InfoMsg, OwnedExchange::Info, ()),
        ]);

        if let [going, well, err, info] = &mut answers[..] {
            assert_text_answer("whats going on", going);
            assert_text_answer("well then", well);
            assert_text_answer("", err);
            assert_text_answer("", info);
        } else {
            panic!("received wrong size {len}!", len = answers.len())
        }
    }

    #[cfg(feature = "linux-pam-ext")]
    fn test_round_trip_linux() {
        use crate::conv::{BinaryData, BinaryQAndA, RadioQAndA};
        let binary_msg = {
            let qa = BinaryQAndA::new((&[][..], 0));
            qa.set_answer(Ok(BinaryData::new(vec![1, 2, 3], 99)));
            OwnedExchange::BinaryPrompt(qa)
        };
        let mut answers = round_trip(vec![
            binary_msg,
            answered!(RadioQAndA, OwnedExchange::RadioPrompt, "beep boop".into()),
        ]);

        if let [bin, radio] = &mut answers[..] {
            let up = unsafe { BinaryAnswer::upcast(bin) };
            assert_eq!((&[1, 2, 3][..], 99), up.contents().unwrap());
            up.zero_contents();
            assert_eq!((&[][..], 0), up.contents().unwrap());

            assert_text_answer("beep boop", radio);
        } else {
            panic!("received wrong size {len}!", len = answers.len())
        }
    }

    #[test]
    fn test_text_answer() {
        let mut answer: CHeapBox<Answer> = CHeapBox::default();
        TextAnswer::fill(&mut answer, "hello".as_ref()).unwrap();
        let zeroth_text = unsafe { TextAnswer::upcast(&mut answer) };
        let data = zeroth_text.contents().expect("valid");
        assert_eq!("hello", data);
        zeroth_text.zero_contents();
        zeroth_text.zero_contents();
    }

    #[test]
    #[should_panic]
    fn test_text_answer_nul() {
        TextAnswer::fill(&mut CHeapBox::default(), "hell\0".as_ref())
            .expect_err("should error; contains nul");
    }

    #[test]
    fn test_binary_answer() {
        use crate::conv::BinaryData;
        let mut answer: CHeapBox<Answer> = CHeapBox::default();
        let real_data = BinaryData::new([1, 2, 3, 4, 5, 6, 7, 8], 9);
        BinaryAnswer::fill(&mut answer, (&real_data).into()).expect("alloc should succeed");
        let bin_answer = unsafe { BinaryAnswer::upcast(&mut answer) };
        assert_eq!(real_data, bin_answer.contents().unwrap().into());
    }

    #[test]
    #[ignore]
    fn test_binary_answer_too_big() {
        let big_data: Vec<u8> = vec![0xFFu8; 0x1_0000_0001];
        let mut answer: CHeapBox<Answer> = CHeapBox::default();
        BinaryAnswer::fill(&mut answer, (&big_data, 100)).expect_err("this is too big!");
    }
}
