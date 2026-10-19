//! PAC script execution using Windows Script Host and the JScript engine.

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use windows::Win32::Foundation::{
    DISP_E_BADPARAMCOUNT, DISP_E_MEMBERNOTFOUND, DISP_E_UNKNOWNNAME, E_INVALIDARG, E_NOTIMPL, E_POINTER,
    TYPE_E_ELEMENTNOTFOUND,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSIDFromProgID, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    DISPATCH_FLAGS, DISPATCH_METHOD, DISPPARAMS, EXCEPINFO, IDispatch, IDispatch_Impl, ITypeInfo,
};
#[cfg(target_pointer_width = "32")]
use windows::Win32::System::Diagnostics::Debug::ActiveScript::IActiveScriptParse32 as IActiveScriptParse;
#[cfg(target_pointer_width = "64")]
use windows::Win32::System::Diagnostics::Debug::ActiveScript::IActiveScriptParse64 as IActiveScriptParse;
use windows::Win32::System::Diagnostics::Debug::ActiveScript::{
    IActiveScript, IActiveScriptError, IActiveScriptSite, IActiveScriptSite_Impl, SCRIPTINFO_IUNKNOWN,
    SCRIPTITEM_GLOBALMEMBERS, SCRIPTITEM_ISVISIBLE, SCRIPTSTATE, SCRIPTSTATE_STARTED,
};
use windows::Win32::System::Variant::{VAR_CHANGE_FLAGS, VARIANT, VT_BSTR, VT_NULL, VariantChangeType, VariantClear};
use windows::core::{AsImpl, BSTR, GUID, HSTRING, IUnknown, Interface, OutRef, PCWSTR, Ref, implement};

use super::{PAC_UTILS, natives};
use crate::Error;

// Name of the item whose members are the native functions
const ITEM_NAME: &str = "proxyres";

// Dispatch ids of the native functions
const NATIVES: [(&str, i32); 5] = [
    ("dnsResolve", 1),
    ("dnsResolveEx", 2),
    ("myIpAddress", 3),
    ("myIpAddressEx", 4),
    ("alert", 5),
];

/// Whether the JScript engine is registered.
pub(super) fn is_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    // SAFETY: The program id is a valid null-terminated string.
    *AVAILABLE.get_or_init(|| unsafe { CLSIDFromProgID(&HSTRING::from("JavaScript")).is_ok() })
}

// Owned VARIANT cleared when dropped
struct Variant(VARIANT);

impl Variant {
    fn null() -> Self {
        let mut variant = VARIANT::default();
        // SAFETY: Setting the type of an empty variant to null leaves it valid.
        unsafe { (*variant.Anonymous.Anonymous).vt = VT_NULL };
        Variant(variant)
    }

    fn string(s: &str) -> Self {
        let mut variant = VARIANT::default();
        // SAFETY: The variant owns the string and releases it when cleared.
        unsafe {
            let inner = &mut *variant.Anonymous.Anonymous;
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(s));
        }
        Variant(variant)
    }

    fn into_raw(self) -> VARIANT {
        let variant = ManuallyDrop::new(self);
        // SAFETY: Ownership of the variant moves to the caller without clearing it.
        unsafe { std::ptr::read(&variant.0) }
    }
}

impl Drop for Variant {
    fn drop(&mut self) {
        // SAFETY: The variant is owned by this wrapper.
        unsafe {
            let _ = VariantClear(&mut self.0);
        }
    }
}

// Read a variant as a string if it is one
fn variant_to_string(variant: &VARIANT) -> Option<String> {
    // SAFETY: The string member is only read when the variant holds a string.
    unsafe {
        let inner = &variant.Anonymous.Anonymous;
        (inner.vt == VT_BSTR).then(|| inner.Anonymous.bstrVal.to_string())
    }
}

// Convert any variant to a string the way the script engine would
fn coerce_to_string(variant: &VARIANT) -> String {
    if let Some(s) = variant_to_string(variant) {
        return s;
    }
    let mut converted = Variant(VARIANT::default());
    // SAFETY: The source variant is valid and the destination is owned.
    match unsafe { VariantChangeType(&mut converted.0, variant, VAR_CHANGE_FLAGS(0), VT_BSTR) } {
        Ok(()) => variant_to_string(&converted.0).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

// Take the description from exception info, releasing its strings
fn take_exception_message(excep_info: &mut EXCEPINFO) -> Option<String> {
    // SAFETY: The strings are owned by the caller once the exception info has been filled in.
    unsafe {
        let description = ManuallyDrop::take(&mut excep_info.bstrDescription);
        drop(ManuallyDrop::take(&mut excep_info.bstrSource));
        drop(ManuallyDrop::take(&mut excep_info.bstrHelpFile));
        (!description.is_empty()).then(|| description.to_string())
    }
}

/// Dispatch interface exposing the native functions to scripts.
#[implement(IDispatch)]
struct Natives;

impl IDispatch_Impl for Natives_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(0)
    }

    fn GetTypeInfo(&self, _itinfo: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        Err(E_NOTIMPL.into())
    }

    fn GetIDsOfNames(
        &self,
        _riid: *const GUID,
        rgsznames: *const PCWSTR,
        cnames: u32,
        _lcid: u32,
        rgdispid: *mut i32,
    ) -> windows::core::Result<()> {
        if rgsznames.is_null() || rgdispid.is_null() {
            return Err(E_POINTER.into());
        }
        let mut result = Ok(());
        for i in 0..cnames as usize {
            // SAFETY: The engine passes cnames names and room for cnames ids.
            unsafe {
                let name = (*rgsznames.add(i)).to_string().unwrap_or_default();
                let id = NATIVES.iter().find(|(native, _)| *native == name).map(|(_, id)| *id);
                *rgdispid.add(i) = id.unwrap_or(-1);
                if id.is_none() {
                    result = Err(DISP_E_UNKNOWNNAME.into());
                }
            }
        }
        result
    }

    fn Invoke(
        &self,
        dispidmember: i32,
        _riid: *const GUID,
        _lcid: u32,
        _wflags: DISPATCH_FLAGS,
        pdispparams: *const DISPPARAMS,
        pvarresult: *mut VARIANT,
        _pexcepinfo: *mut EXCEPINFO,
        _puargerr: *mut u32,
    ) -> windows::core::Result<()> {
        // SAFETY: The engine passes valid parameters for the duration of the call.
        let params = unsafe { pdispparams.as_ref() }.ok_or(windows::core::Error::from(E_POINTER))?;
        // Arguments are passed in reverse order
        let first_arg = || -> windows::core::Result<String> {
            if params.cArgs < 1 || params.rgvarg.is_null() {
                return Err(DISP_E_BADPARAMCOUNT.into());
            }
            // SAFETY: The engine passes cArgs arguments.
            Ok(coerce_to_string(unsafe {
                &*params.rgvarg.add(params.cArgs as usize - 1)
            }))
        };
        let result = match dispidmember {
            1 => match natives::dns_resolve(&first_arg()?) {
                Some(address) => Variant::string(&address),
                None => Variant::null(),
            },
            2 => Variant::string(&natives::dns_resolve_ex(&first_arg()?)),
            3 => Variant::string(&natives::my_ip_address()),
            4 => Variant::string(&natives::my_ip_address_ex()),
            5 => {
                natives::alert(&first_arg().unwrap_or_default());
                Variant::null()
            }
            _ => return Err(DISP_E_MEMBERNOTFOUND.into()),
        };
        // SAFETY: The result pointer is null or points to a variant owned by the engine.
        if let Some(pvarresult) = unsafe { pvarresult.as_mut() } {
            *pvarresult = result.into_raw();
        }
        Ok(())
    }
}

/// Script site that provides the native functions and records script errors.
#[implement(IActiveScriptSite)]
struct Site {
    natives: IDispatch,
    error: RefCell<Option<String>>,
}

impl IActiveScriptSite_Impl for Site_Impl {
    fn GetLCID(&self) -> windows::core::Result<u32> {
        Err(E_NOTIMPL.into())
    }

    fn GetItemInfo(
        &self,
        pstrname: &PCWSTR,
        dwreturnmask: u32,
        ppiunkitem: OutRef<IUnknown>,
        ppti: OutRef<ITypeInfo>,
    ) -> windows::core::Result<()> {
        // SAFETY: The engine passes a valid name.
        let name = unsafe { pstrname.to_string() }.unwrap_or_default();
        if dwreturnmask & SCRIPTINFO_IUNKNOWN != 0 {
            if ppiunkitem.is_null() {
                return Err(E_INVALIDARG.into());
            }
            if name == ITEM_NAME {
                let _ = ppti.write(None);
                return ppiunkitem.write(Some(self.natives.cast()?));
            }
            ppiunkitem.write(None)?;
        }
        let _ = ppti.write(None);
        Err(TYPE_E_ELEMENTNOTFOUND.into())
    }

    fn GetDocVersionString(&self) -> windows::core::Result<BSTR> {
        Err(E_NOTIMPL.into())
    }

    fn OnScriptTerminate(
        &self,
        _pvarresult: *const VARIANT,
        _pexcepinfo: *const EXCEPINFO,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnStateChange(&self, _ssscriptstate: SCRIPTSTATE) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnScriptError(&self, pscripterror: Ref<IActiveScriptError>) -> windows::core::Result<()> {
        let Some(error) = pscripterror.as_ref() else {
            return Ok(());
        };
        let mut excep_info = EXCEPINFO::default();
        // SAFETY: The error interface is valid for the duration of the call.
        if unsafe { error.GetExceptionInfo(&mut excep_info) }.is_ok() {
            let message = take_exception_message(&mut excep_info);
            *self.error.borrow_mut() = Some(message.unwrap_or_else(|| "script error".to_string()));
        }
        Ok(())
    }

    fn OnEnterScript(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnLeaveScript(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

// Initializes COM for the current thread and uninitializes it only if the call succeeded
struct ComApartment(bool);

impl ComApartment {
    fn new() -> Self {
        // SAFETY: Initializing COM has no preconditions.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // A thread already initialized for a different concurrency model returns RPC_E_CHANGED_MODE
        // and must not be uninitialized
        ComApartment(result.is_ok())
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: Balances the successful initialization in new.
            unsafe { CoUninitialize() };
        }
    }
}

// Script engine closed when dropped
struct Engine {
    script: IActiveScript,
    parse: IActiveScriptParse,
    site: IActiveScriptSite,
}

impl Engine {
    fn new() -> windows::core::Result<Self> {
        let natives: IDispatch = Natives.into();
        let site: IActiveScriptSite = Site {
            natives,
            error: RefCell::new(None),
        }
        .into();
        // SAFETY: The engine is created and configured through its documented interfaces.
        unsafe {
            let clsid = CLSIDFromProgID(&HSTRING::from("JavaScript"))?;
            let script: IActiveScript = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?;
            let parse: IActiveScriptParse = script.cast()?;
            script.SetScriptSite(&site)?;
            parse.InitNew()?;
            script.AddNamedItem(
                &HSTRING::from(ITEM_NAME),
                SCRIPTITEM_GLOBALMEMBERS | SCRIPTITEM_ISVISIBLE,
            )?;
            script.SetScriptState(SCRIPTSTATE_STARTED)?;
            Ok(Engine { script, parse, site })
        }
    }

    // Take the last error reported to the site
    fn site_error(&self) -> Option<String> {
        // SAFETY: The site was created from Site in new.
        let site = unsafe { self.site.as_impl() };
        site.error.borrow_mut().take()
    }

    fn script_error(&self, error: windows::core::Error, excep_info: Option<&mut EXCEPINFO>) -> Error {
        let message = excep_info
            .and_then(take_exception_message)
            .or_else(|| self.site_error())
            .unwrap_or_else(|| error.message());
        Error::Script(message)
    }

    fn parse_text(&self, code: &str) -> Result<(), Error> {
        let mut excep_info = EXCEPINFO::default();
        // SAFETY: The code and exception info are valid for the duration of the call.
        let result = unsafe {
            self.parse.ParseScriptText(
                &HSTRING::from(code),
                &HSTRING::from(ITEM_NAME),
                None,
                PCWSTR::null(),
                0,
                0,
                0,
                std::ptr::null_mut(),
                &mut excep_info,
            )
        };
        result.map_err(|error| self.script_error(error, Some(&mut excep_info)))
    }

    fn find_proxy_for_url(&self, url: &str, host: &str) -> Result<String, Error> {
        // SAFETY: The dispatch interface and parameters are valid for the duration of the calls.
        unsafe {
            let dispatch = self
                .script
                .GetScriptDispatch(PCWSTR::null())
                .map_err(|error| Error::Script(error.message()))?;
            let name = HSTRING::from("FindProxyForURL");
            let names = [PCWSTR(name.as_ptr())];
            let mut dispid = 0;
            dispatch
                .GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0, &mut dispid)
                .map_err(|_| Error::Script("FindProxyForURL is not a function".to_string()))?;

            // Arguments are passed in reverse order
            let mut args = [Variant::string(host), Variant::string(url)];
            let params = DISPPARAMS {
                rgvarg: args.as_mut_ptr() as *mut VARIANT,
                rgdispidNamedArgs: std::ptr::null_mut(),
                cArgs: args.len() as u32,
                cNamedArgs: 0,
            };
            let mut result = Variant(VARIANT::default());
            let mut excep_info = EXCEPINFO::default();
            dispatch
                .Invoke(
                    dispid,
                    &GUID::zeroed(),
                    0,
                    DISPATCH_METHOD,
                    &params,
                    Some(&mut result.0),
                    Some(&mut excep_info),
                    None,
                )
                .map_err(|error| self.script_error(error, Some(&mut excep_info)))?;
            variant_to_string(&result.0)
                .ok_or_else(|| Error::Script("FindProxyForURL did not return a string".to_string()))
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: Closing the engine releases its references to the site.
        unsafe {
            let _ = self.script.Close();
        }
    }
}

/// Evaluates the PAC script and calls `FindProxyForURL(url, host)`.
pub(super) fn find_proxy_for_url(script: &str, url: &str, host: &str) -> Result<String, Error> {
    let _apartment = ComApartment::new();
    let engine = Engine::new().map_err(|error| Error::Script(error.message()))?;
    // Load Mozilla's PAC helper functions and the PAC script
    engine.parse_text(PAC_UTILS)?;
    engine.parse_text(script)?;
    engine.find_proxy_for_url(url, host)
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::S_FALSE;
    use windows::Win32::System::Com::COINIT_MULTITHREADED;

    use super::*;

    #[test]
    fn keeps_caller_com_initialization() {
        if !is_available() {
            return;
        }
        std::thread::spawn(|| {
            // SAFETY: Each successful initialization is balanced by an uninitialization.
            unsafe {
                assert!(CoInitializeEx(None, COINIT_MULTITHREADED).is_ok());
                let script = "function FindProxyForURL(url, host) { return 'DIRECT'; }";
                assert_eq!(find_proxy_for_url(script, "http://a/", "a").as_deref(), Ok("DIRECT"));
                // The caller's multithreaded apartment is still initialized
                assert_eq!(CoInitializeEx(None, COINIT_MULTITHREADED), S_FALSE);
                CoUninitialize();
                CoUninitialize();
            }
        })
        .join()
        .unwrap();
    }
}
