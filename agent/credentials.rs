//! OS-backed secrets. References are immutable: a failed config save cannot destroy the old key.
use anyhow::{Context, Result, bail};
use std::path::Path;

pub fn store(config: &Path, secret: &str) -> Result<String> {
    if secret.is_empty() || secret.len() > 4096 || secret.contains(['\0', '\n', '\r']) {
        bail!("API key must contain 1–4096 bytes without control line breaks")
    }
    let reference = uuid::Uuid::new_v4().to_string();
    backend::store(config, &reference, secret)
        .map_err(|_| anyhow::anyhow!("system credential storage failed; original configuration was preserved (unlock the OS credential store and retry)"))?;
    if load(config, &reference)? != secret {
        bail!("system credential verification failed; original configuration was preserved")
    }
    Ok(reference)
}

pub fn load(config: &Path, reference: &str) -> Result<String> {
    let id = uuid::Uuid::parse_str(reference).context("invalid credential reference")?;
    if id.to_string() != reference {
        bail!("invalid credential reference")
    }
    backend::load(config, reference).map_err(|_| {
        anyhow::anyhow!(
            "system credential unavailable; unlock the OS credential store or re-enter the API key"
        )
    })
}

#[cfg(windows)]
mod backend {
    use super::*;
    use crate::util;
    use std::{fs, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };

    fn path(config: &Path, reference: &str) -> Result<std::path::PathBuf> {
        let dir = config.with_extension("credentials");
        fs::create_dir_all(&dir)?;
        let path = dir.join(reference);
        if util::is_link(&dir) || util::is_link(&path) {
            bail!("linked credential storage")
        }
        Ok(path)
    }

    fn crypt(bytes: &[u8], protect: bool) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len().try_into()?,
            pbData: bytes.as_ptr().cast_mut(),
        };
        let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
        // Current-user DPAPI, never CRYPTPROTECT_LOCAL_MACHINE. No UI or secret-bearing errors.
        let success = unsafe {
            if protect {
                CryptProtectData(
                    &input,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            }
        };
        if success == 0 {
            bail!("DPAPI failed")
        }
        let result =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
        unsafe {
            std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
            LocalFree(output.pbData.cast());
        }
        Ok(result)
    }

    pub fn store(config: &Path, reference: &str, secret: &str) -> Result<()> {
        util::atomic_write(&path(config, reference)?, &crypt(secret.as_bytes(), true)?)
    }
    pub fn load(config: &Path, reference: &str) -> Result<String> {
        let path = path(config, reference)?;
        if path.metadata()?.len() > 65_536 {
            bail!("invalid encrypted credential size")
        }
        Ok(String::from_utf8(crypt(&fs::read(path)?, false)?)?)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn account(config: &Path, reference: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}:{reference}",
        Sha256::digest(crate::util::canonical(config)?.to_string_lossy().as_bytes())
    ))
}

#[cfg(target_os = "macos")]
mod backend {
    use super::*;
    use std::{ffi::c_void, ptr};
    const SERVICE: &[u8] = b"org.minicode.credentials";
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainAddGenericPassword(
            keychain: *const c_void,
            service_len: u32,
            service: *const u8,
            account_len: u32,
            account: *const u8,
            password_len: u32,
            password: *const u8,
            item: *mut *mut c_void,
        ) -> i32;
        fn SecKeychainFindGenericPassword(
            keychain: *const c_void,
            service_len: u32,
            service: *const u8,
            account_len: u32,
            account: *const u8,
            password_len: *mut u32,
            password: *mut *mut u8,
            item: *mut *mut c_void,
        ) -> i32;
        fn SecKeychainItemFreeContent(attributes: *const c_void, data: *mut c_void) -> i32;
    }
    pub fn store(config: &Path, reference: &str, secret: &str) -> Result<()> {
        let account = super::account(config, reference)?;
        let status = unsafe {
            SecKeychainAddGenericPassword(
                ptr::null(),
                SERVICE.len() as u32,
                SERVICE.as_ptr(),
                account.len() as u32,
                account.as_ptr(),
                secret.len() as u32,
                secret.as_ptr(),
                ptr::null_mut(),
            )
        };
        if status != 0 {
            bail!("Keychain write failed")
        }
        Ok(())
    }
    pub fn load(config: &Path, reference: &str) -> Result<String> {
        let account = super::account(config, reference)?;
        let mut len = 0;
        let mut data = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null(),
                SERVICE.len() as u32,
                SERVICE.as_ptr(),
                account.len() as u32,
                account.as_ptr(),
                &mut len,
                &mut data,
                ptr::null_mut(),
            )
        };
        if status != 0 {
            bail!("Keychain read failed")
        }
        let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec();
        unsafe {
            SecKeychainItemFreeContent(ptr::null(), data.cast());
        }
        Ok(String::from_utf8(bytes)?)
    }
}

#[cfg(target_os = "linux")]
mod backend {
    use super::*;
    use std::{
        io::{Read, Write},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    fn run(config: &Path, reference: &str, secret: Option<&str>) -> Result<String> {
        let account = super::account(config, reference)?;
        // Fixed OS libsecret client, never resolved from workspace PATH. Secrets use stdin only.
        let mut command = Command::new("/usr/bin/secret-tool");
        if secret.is_some() {
            command.args(["store", "--label=MiniCode API key"]);
        } else {
            command.arg("lookup");
        }
        let mut child = command
            .args([
                "application",
                "org.minicode.credentials",
                "account",
                &account,
            ])
            .stdin(if secret.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(secret) = secret {
            let result = child
                .stdin
                .take()
                .context("credential input unavailable")?
                .write_all(secret.as_bytes());
            if result.is_err() {
                let _ = child.kill();
                let _ = child.wait();
                result?;
            }
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    bail!("Secret Service operation failed")
                }
                let mut result = String::new();
                child
                    .stdout
                    .take()
                    .context("credential output unavailable")?
                    .take(8192)
                    .read_to_string(&mut result)?;
                return Ok(result.trim_end_matches('\n').to_string());
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Secret Service timed out")
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn store(config: &Path, reference: &str, secret: &str) -> Result<()> {
        run(config, reference, Some(secret)).map(|_| ())
    }
    pub fn load(config: &Path, reference: &str) -> Result<String> {
        run(config, reference, None)
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod backend {
    use super::*;
    pub fn store(_: &Path, _: &str, _: &str) -> Result<()> {
        bail!("unsupported OS credential storage")
    }
    pub fn load(_: &Path, _: &str) -> Result<String> {
        bail!("unsupported OS credential storage")
    }
}
