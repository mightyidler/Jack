//! Undocumented ABI isolated here. Slots match EarTrumpet commit
//! 48e8958809ac736efdf6878656ab2c66ce323b7c (see docs/policy-compatibility.md).
//! Unused slots are never called. Do not reorder them or expose ClearAll.
use std::ffi::c_void;
use windows::Win32::Media::Audio::{EDataFlow, ERole};
use windows::core::{
    GUID, HRESULT, HSTRING, IInspectable_Vtbl, IUnknown_Vtbl, Interface, PCWSTR, Result,
};

windows_core::imp::define_interface!(
    PolicyConfig,
    PolicyConfigVtbl,
    0xf8679f50_850a_41cf_9c72_430f290290c8
);
#[repr(C)]
pub struct PolicyConfigVtbl {
    base: IUnknown_Vtbl,
    unused: [unsafe extern "system" fn(); 10],
    pub(super) set_default_endpoint:
        unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT,
    unused_visibility: unsafe extern "system" fn(),
}
pub const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
impl PolicyConfig {
    pub(super) fn set_default(&self, endpoint: &HSTRING, role: ERole) -> Result<()> {
        // SAFETY: exact queried IID and vtable slot; endpoint is live UTF-16 for this call.
        unsafe {
            (self.vtable().set_default_endpoint)(self.as_raw(), PCWSTR(endpoint.as_ptr()), role)
                .ok()
        }
    }
}
windows_core::imp::define_interface!(
    AppPolicy21H2,
    AppPolicyVtbl,
    0xab3d4648_e242_459f_b02f_541c70306324
);
windows_core::imp::define_interface!(
    AppPolicyDownlevel,
    AppPolicyVtbl,
    0x2a59116d_6c4f_45e0_a74f_707e3fef9258
);
#[repr(C)]
pub struct AppPolicyVtbl {
    base: IInspectable_Vtbl,
    unused: [unsafe extern "system" fn(); 19],
    pub(super) set_endpoint:
        unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut c_void) -> HRESULT,
    get_endpoint:
        unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut *mut c_void) -> HRESULT,
    unused_clear_all: unsafe extern "system" fn(),
}
pub(super) enum AppPolicy {
    Modern(AppPolicy21H2),
    Downlevel(AppPolicyDownlevel),
}
impl AppPolicy {
    fn raw(&self) -> (*mut c_void, &AppPolicyVtbl) {
        match self {
            Self::Modern(i) => (i.as_raw(), i.vtable()),
            Self::Downlevel(i) => (i.as_raw(), i.vtable()),
        }
    }
    pub(super) fn variant(&self) -> &'static str {
        match self {
            Self::Modern(_) => "21h2",
            Self::Downlevel(_) => "downlevel",
        }
    }
    pub(super) fn get(&self, pid: u32, flow: EDataFlow, role: ERole) -> Result<HSTRING> {
        let (object, vtable) = self.raw();
        let mut value = HSTRING::new();
        // SAFETY: HSTRING is repr(transparent) over an owned ABI handle; Drop releases it.
        unsafe {
            (vtable.get_endpoint)(
                object,
                pid,
                flow,
                role,
                std::ptr::from_mut(&mut value).cast(),
            )
            .ok()?;
        }
        Ok(value)
    }
    pub(super) fn set(
        &self,
        pid: u32,
        flow: EDataFlow,
        role: ERole,
        value: &HSTRING,
    ) -> Result<()> {
        let (object, vtable) = self.raw();
        // SAFETY: borrow the HSTRING ABI handle without transferring its ownership.
        unsafe {
            (vtable.set_endpoint)(
                object,
                pid,
                flow,
                role,
                std::mem::transmute_copy::<HSTRING, *mut c_void>(value),
            )
            .ok()
        }
    }
}
