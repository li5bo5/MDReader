//! 极轻量自定义控件 UI Automation 辅助模块（按需懒加载 `uiautomationcore.dll`）。
//!
//! 说明：
//! - 仅在真正收到 `WM_GETOBJECT(UiaRootObjectId)` 时才动态加载 `uiautomationcore.dll`，
//!   避免日常纯阅读场景下预先映射 ~4 MB UIA 运行时进入工作集；
//! - 标准 Win32 控件（`BUTTON`、`EDIT`、`STATIC`、`RICHEDIT50W`）由系统原生代理提供完整树与 Pattern；
//! - 仅对自定义画布 `MDReaderCanvas` 使用标准 `UiaHostProviderFromHwnd(hwnd)` 暴露 `IDC_READER`。

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IRawElementProviderSimple, IRawElementProviderSimple_Impl, ProviderOptions,
    ProviderOptions_ServerSideProvider, UIA_AutomationIdPropertyId, UIA_ControlTypePropertyId,
    UIA_NamePropertyId, UIA_PROPERTY_ID, UIA_PaneControlTypeId, UiaRootObjectId,
};
use windows::core::{BSTR, HRESULT, IUnknown, Interface, Result, implement, s, w};

type UiaHostProviderFromHwndFn =
    unsafe extern "system" fn(HWND, *mut *mut c_void) -> HRESULT;
type UiaReturnRawElementProviderFn =
    unsafe extern "system" fn(HWND, WPARAM, LPARAM, *mut c_void) -> LRESULT;

fn lazy_uia_host_provider(hwnd: HWND) -> Result<IRawElementProviderSimple> {
    unsafe {
        let hmod = LoadLibraryW(w!("uiautomationcore.dll"))?;
        let Some(proc) = GetProcAddress(hmod, s!("UiaHostProviderFromHwnd")) else {
            return Err(windows::core::Error::empty());
        };
        let func: UiaHostProviderFromHwndFn = std::mem::transmute(proc);
        let mut raw: *mut c_void = std::ptr::null_mut();
        func(hwnd, &mut raw).ok()?;
        if raw.is_null() {
            return Err(windows::core::Error::empty());
        }
        Ok(IRawElementProviderSimple::from_raw(raw))
    }
}

fn lazy_uia_return_provider(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    provider: &IRawElementProviderSimple,
) -> Option<LRESULT> {
    unsafe {
        let hmod = LoadLibraryW(w!("uiautomationcore.dll")).ok()?;
        let proc = GetProcAddress(hmod, s!("UiaReturnRawElementProvider"))?;
        let func: UiaReturnRawElementProviderFn = std::mem::transmute(proc);
        Some(func(hwnd, wparam, lparam, provider.as_raw()))
    }
}

/// 自定义阅读画布的轻量 UIA Provider（通过 `UiaHostProviderFromHwnd` 完整保留原生 HWND 树结构）。
#[implement(IRawElementProviderSimple)]
struct ReaderCanvasProvider {
    hwnd: HWND,
    automation_id: String,
    name: String,
}

impl IRawElementProviderSimple_Impl for ReaderCanvasProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(
        &self,
        _patternid: windows::Win32::UI::Accessibility::UIA_PATTERN_ID,
    ) -> Result<IUnknown> {
        Err(windows::core::Error::empty())
    }

    fn GetPropertyValue(&self, propertyid: UIA_PROPERTY_ID) -> Result<VARIANT> {
        if propertyid == UIA_AutomationIdPropertyId {
            Ok(VARIANT::from(BSTR::from(&self.automation_id)))
        } else if propertyid == UIA_NamePropertyId {
            Ok(VARIANT::from(BSTR::from(&self.name)))
        } else if propertyid == UIA_ControlTypePropertyId {
            Ok(VARIANT::from(UIA_PaneControlTypeId.0 as i32))
        } else {
            Ok(VARIANT::default())
        }
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        lazy_uia_host_provider(self.hwnd)
    }
}

/// 响应自定义画布窗口 `WM_GETOBJECT`，按需加载 UIA 并暴露 `IDC_READER`。
pub fn handle_wm_getobject(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    automation_id: &str,
) -> Option<LRESULT> {
    if lparam.0 as i32 == UiaRootObjectId {
        let provider: IRawElementProviderSimple = ReaderCanvasProvider {
            hwnd,
            automation_id: automation_id.to_string(),
            name: "Markdown 阅读画布".to_string(),
        }
        .into();
        return lazy_uia_return_provider(hwnd, wparam, lparam, &provider);
    }
    None
}
