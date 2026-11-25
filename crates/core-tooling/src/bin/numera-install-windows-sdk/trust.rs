use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;
use windows_sys::Win32::Security::Cryptography::{
    CERT_NAME_ATTR_TYPE, CERT_NAME_RDN_TYPE, CERT_X500_NAME_STR, CertGetNameStringW,
    szOID_ORGANIZATION_NAME,
};
use windows_sys::Win32::Security::WinTrust::*;
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

pub fn publisher(path: &Path) -> Result<String> {
    let path = wide(path);
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path.as_ptr(),
        ..Default::default()
    };
    let mut policy = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        ..Default::default()
    };
    // SAFETY: All structures and terminated strings are live until the trust state is closed.
    unsafe {
        let status = WinVerifyTrust(null_mut(), &mut policy, (&raw mut data).cast());
        let signer = if status == 0 {
            verified_publisher(&data)
        } else {
            Err(format!("SDK installer Authenticode verification failed ({status:#x})").into())
        };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(null_mut(), &mut policy, (&raw mut data).cast());
        signer
    }
}

unsafe fn verified_publisher(data: &WINTRUST_DATA) -> Result<String> {
    // SAFETY: WinVerifyTrust initialized the state. Provider-owned objects remain live
    // until close, and each nullable intermediate pointer is checked before use.
    let context = unsafe {
        let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
        if provider.is_null() {
            return Err("SDK installer has no Authenticode state".into());
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
        if signer.is_null() {
            return Err("SDK installer is missing a verified signer".into());
        }
        let cert = WTHelperGetProvCertFromChain(signer, 0);
        if cert.is_null() || (*cert).pCert.is_null() {
            return Err("SDK installer is missing a verified certificate".into());
        }
        (*cert).pCert
    };

    let name = |kind, parameter| -> Result<String> {
        let mut buffer = [0u16; 2048];
        // SAFETY: The validated certificate context and supplied parameter are live.
        // The writable buffer size is passed in UTF-16 code units.
        let len = unsafe {
            CertGetNameStringW(
                context,
                kind,
                0,
                parameter,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        } as usize;
        if !(2..=buffer.len()).contains(&len) {
            return Err("SDK signer certificate attribute is missing".into());
        }
        Ok(String::from_utf16(&buffer[..len - 1])?)
    };
    if name(CERT_NAME_ATTR_TYPE, szOID_ORGANIZATION_NAME.cast())? != "Microsoft Corporation" {
        return Err("SDK installer publisher verification failed".into());
    }
    let format = CERT_X500_NAME_STR;
    name(CERT_NAME_RDN_TYPE, (&raw const format).cast())
}

pub struct DebuggerVersion {
    pub parts: [u32; 4],
    pub file_version: String,
}

fn resource_version(bytes: &[u8]) -> Result<String> {
    let mut pointer = null_mut::<c_void>();
    let mut length = 0;
    // SAFETY: The resource bytes came from GetFileVersionInfoW and stay live.
    // Translation consists of aligned or unaligned language and code-page WORDs.
    let translation = unsafe {
        if VerQueryValueW(
            bytes.as_ptr().cast(),
            windows_sys::core::w!("\\VarFileInfo\\Translation"),
            &mut pointer,
            &mut length,
        ) != 0
            && !pointer.is_null()
            && length >= 4
        {
            let words = pointer.cast::<u16>();
            Some((words.read_unaligned(), words.add(1).read_unaligned()))
        } else {
            None
        }
    };
    for (language, codepage) in
        translation
            .into_iter()
            .chain([(0x0409, 0x04b0), (0x0409, 0x04e4), (0x0409, 0)])
    {
        let subkey: Vec<u16> =
            format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\FileVersion")
                .encode_utf16()
                .chain(Some(0))
                .collect();
        // SAFETY: subkey is NUL-terminated. On success length counts UTF-16
        // units in the returned string, which is borrowed only within bytes.
        let resource = unsafe {
            if VerQueryValueW(
                bytes.as_ptr().cast(),
                subkey.as_ptr(),
                &mut pointer,
                &mut length,
            ) == 0
                || pointer.is_null()
                || !(2..=2048).contains(&length)
            {
                continue;
            }
            let units = pointer.cast::<u16>();
            (0..length as usize - 1)
                .map(|index| units.add(index).read_unaligned())
                .collect::<Vec<_>>()
        };
        return Ok(String::from_utf16(&resource)?);
    }
    Err("SDK debugger FileVersion string is unavailable".into())
}

pub fn debugger_version(path: &Path) -> Result<DebuggerVersion> {
    let path = wide(path);
    let mut reserved = 0;
    // SAFETY: The path is a NUL-terminated UTF-16 buffer and reserved is writable.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut reserved) };
    if !(1..=1024 * 1024).contains(&size) {
        return Err("SDK debugger version resource is unavailable".into());
    }
    let mut bytes = vec![0u8; size as usize];
    let mut result = null_mut::<c_void>();
    let mut length = 0;
    // SAFETY: The version buffer has the requested size. VerQueryValueW returns a
    // pointer into that buffer, read below while the buffer is still live.
    let fixed = unsafe {
        if GetFileVersionInfoW(path.as_ptr(), 0, size, bytes.as_mut_ptr().cast()) == 0
            || VerQueryValueW(
                bytes.as_ptr().cast(),
                windows_sys::core::w!("\\"),
                &mut result,
                &mut length,
            ) == 0
            || result.is_null()
            || length < size_of::<VS_FIXEDFILEINFO>() as u32
        {
            return Err("SDK debugger version resource is malformed".into());
        }
        result.cast::<VS_FIXEDFILEINFO>().read_unaligned()
    };
    if fixed.dwSignature != 0xFEEF04BD {
        return Err("SDK debugger version resource has an invalid signature".into());
    }
    Ok(DebuggerVersion {
        parts: [
            fixed.dwFileVersionMS >> 16,
            fixed.dwFileVersionMS & 0xFFFF,
            fixed.dwFileVersionLS >> 16,
            fixed.dwFileVersionLS & 0xFFFF,
        ],
        file_version: resource_version(&bytes)?,
    })
}
