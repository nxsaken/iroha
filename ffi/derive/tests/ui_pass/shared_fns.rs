use std::{cmp::Ordering, mem::MaybeUninit};

use iroha_ffi::{
    ffi_export, gen_ffi_impl, handles, AsReprCRef, Handle, IntoFfi, TryFromFfi, TryFromReprC,
};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, IntoFfi, TryFromFfi)]
pub struct FfiStruct1 {
    name: String,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, IntoFfi, TryFromFfi)]
pub struct FfiStruct2 {
    name: String,
}

handles! {0, FfiStruct1, FfiStruct2}
gen_ffi_impl! {Drop: FfiStruct1, FfiStruct2}
gen_ffi_impl! {Clone: FfiStruct1, FfiStruct2}
gen_ffi_impl! {Eq: FfiStruct1, FfiStruct2}
gen_ffi_impl! {Ord: FfiStruct1, FfiStruct2}

#[ffi_export]
impl FfiStruct1 {
    /// New
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

fn main() {
    let name = String::from("X");

    let ffi_struct1: FfiStruct1 = unsafe {
        let mut ffi_struct = MaybeUninit::<*mut FfiStruct1>::uninit();
        let name = IntoFfi::into_ffi(name);
        FfiStruct1__new(name.as_ref(), ffi_struct.as_mut_ptr());
        TryFromReprC::try_from_repr_c(ffi_struct.assume_init(), &mut ()).unwrap()
    };

    unsafe {
        let cloned: FfiStruct1 = {
            let mut cloned: MaybeUninit<*mut FfiStruct1> = MaybeUninit::uninit();

            __clone(
                FfiStruct1::ID,
                (&ffi_struct1).into_ffi().cast(),
                cloned.as_mut_ptr().cast(),
            );

            TryFromReprC::try_from_repr_c(cloned.assume_init(), &mut ()).unwrap()
        };

        let mut is_equal: MaybeUninit<u8> = MaybeUninit::uninit();
        __eq(
            FfiStruct1::ID,
            (&ffi_struct1).into_ffi().cast(),
            (&cloned).into_ffi().cast(),
            is_equal.as_mut_ptr(),
        );
        assert_eq!(
            true,
            TryFromReprC::try_from_repr_c(is_equal.assume_init(), &mut ()).unwrap()
        );

        let mut ordering: MaybeUninit<i8> = MaybeUninit::uninit();
        __ord(
            FfiStruct1::ID,
            (&ffi_struct1).into_ffi().cast(),
            (&cloned).into_ffi().cast(),
            ordering.as_mut_ptr(),
        );
        assert_eq!(
            Ordering::Equal,
            TryFromReprC::try_from_repr_c(ordering.assume_init(), &mut ()).unwrap()
        );

        __drop(FfiStruct1::ID, ffi_struct1.into_ffi().cast());
        __drop(FfiStruct1::ID, cloned.into_ffi().cast());
    }
}