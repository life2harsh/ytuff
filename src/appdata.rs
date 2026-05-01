use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
#[cfg(target_os = "windows")]
use std::path::Path;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
use aes_gcm::aead::{generic_array::GenericArray, Aead, KeyInit};
#[cfg(target_os = "windows")]
use aes_gcm::{Aes256Gcm, Nonce};
#[cfg(target_os = "windows")]
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
#[cfg(target_os = "windows")]
use std::collections::BTreeMap;
#[cfg(target_os = "windows")]
use std::ffi::OsString;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStringExt;
#[cfg(target_os = "windows")]
use std::process::Command;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, BOOL, HANDLE};
#[cfg(target_os = "windows")]
use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
#[cfg(target_os = "windows")]
use windows_sys::Win32::Security::{
    DuplicateToken, ImpersonateLoggedOnUser, RevertToSelf, SecurityImpersonation, TOKEN_DUPLICATE,
    TOKEN_QUERY,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::ProcessStatus::{EnumProcesses, K32GetProcessImageFileNameW};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub config_file: PathBuf,
    pub playlists_file: PathBuf,
    pub lyrics_dir: PathBuf,
    pub downloads_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Self {
        let mut config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.push("rustplayer");

        let mut data_dir = dirs::data_local_dir().unwrap_or_else(|| config_dir.clone());
        data_dir.push("rustplayer");

        let mut cache_dir = dirs::cache_dir().unwrap_or_else(|| data_dir.clone());
        cache_dir.push("rustplayer");

        let mut config_file = config_dir.clone();
        config_file.push("config.json");

        let mut playlists_file = data_dir.clone();
        playlists_file.push("playlists.json");

        let mut lyrics_dir = cache_dir.clone();
        lyrics_dir.push("lyrics");

        let mut downloads_dir = data_dir.clone();
        downloads_dir.push("downloads");

        Self {
            config_dir,
            data_dir,
            cache_dir,
            config_file,
            playlists_file,
            lyrics_dir,
            downloads_dir,
        }
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.lyrics_dir,
            &self.downloads_dir,
        ] {
            fs::create_dir_all(dir)
                .with_context(|| format!("Could not create {}", dir.display()))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub quality: String,
    pub scan_paths: Vec<PathBuf>,
    pub autoplay: bool,
    pub lyrics_enabled: bool,
    pub auto_fetch_lyrics: bool,
    pub daemon_addr: String,
    pub downloads_dir: Option<PathBuf>,
    pub youtube_cookie_header: Option<String>,
    pub youtube_cookie_file: Option<PathBuf>,
    pub youtube_auth_user: Option<String>,
    pub start_background_on_boot: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            quality: "high".to_string(),
            scan_paths: Vec::new(),
            autoplay: false,
            lyrics_enabled: true,
            auto_fetch_lyrics: true,
            daemon_addr: "127.0.0.1:38185".to_string(),
            downloads_dir: None,
            youtube_cookie_header: None,
            youtube_cookie_file: None,
            youtube_auth_user: None,
            start_background_on_boot: false,
        }
    }
}

impl AppConfig {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        paths.ensure()?;
        if !paths.config_file.exists() {
            let cfg = Self::default();
            cfg.save(paths)?;
            return Ok(cfg);
        }
        let txt = fs::read_to_string(&paths.config_file)
            .with_context(|| format!("Could not read {}", paths.config_file.display()))?;
        let cfg = serde_json::from_str(&txt)
            .with_context(|| format!("Could not parse {}", paths.config_file.display()))?;
        Ok(cfg)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        paths.ensure()?;
        fs::write(&paths.config_file, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("Could not write {}", paths.config_file.display()))
    }

    pub fn effective_downloads_dir(&self, paths: &AppPaths) -> PathBuf {
        self.downloads_dir
            .clone()
            .unwrap_or_else(|| paths.downloads_dir.clone())
    }

    pub fn cookie_header(&self) -> Result<Option<String>> {
        if let Some(header) = self
            .youtube_cookie_header
            .as_ref()
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            return Ok(Some(header.to_string()));
        }

        let Some(path) = self.youtube_cookie_file.as_ref() else {
            return Ok(browser_cookie_header());
        };
        let txt = fs::read_to_string(path)
            .with_context(|| format!("Could not read cookie source {}", path.display()))?;
        if let Some(header) = parse_cookie_source(&txt) {
            return Ok(Some(header));
        }

        Ok(browser_cookie_header())
    }
}

fn parse_cookie_source(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains('\t') {
        let pairs = trimmed
            .lines()
            .filter(|line| {
                let line = line.trim();
                !line.is_empty() && !line.starts_with('#')
            })
            .filter_map(|line| {
                let cols = line.split('\t').collect::<Vec<_>>();
                if cols.len() >= 7 {
                    Some(format!("{}={}", cols[5], cols[6]))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if !pairs.is_empty() {
            return Some(pairs.join("; "));
        }
    }

    trimmed.contains('=').then(|| trimmed.to_string())
}

#[cfg(target_os = "windows")]
fn browser_cookie_header() -> Option<String> {
    let local = dirs::data_local_dir()?;
    let browsers = [
        local.join("Microsoft").join("Edge").join("User Data"),
        local.join("Google").join("Chrome").join("User Data"),
        local
            .join("BraveSoftware")
            .join("Brave-Browser")
            .join("User Data"),
    ];

    for root in browsers {
        if let Some(header) = chromium_cookie_header(&root) {
            return Some(header);
        }
    }

    None
}

#[cfg(not(target_os = "windows"))]
fn browser_cookie_header() -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
#[derive(Deserialize)]
struct ChromiumCookieRow {
    domain: String,
    name: String,
    plain_value: String,
    encrypted_value_b64: String,
}

#[cfg(target_os = "windows")]
struct ChromiumKeys {
    legacy: Vec<u8>,
    app_bound: Vec<Vec<u8>>,
}

#[cfg(target_os = "windows")]
unsafe extern "system" {
    fn RtlAdjustPrivilege(
        privilege: i32,
        enable: BOOL,
        current_thread: BOOL,
        previous_value: *mut BOOL,
    ) -> i32;
}

#[cfg(target_os = "windows")]
fn chromium_cookie_header(root: &Path) -> Option<String> {
    let keys = chromium_keys(&root.join("Local State"))?;

    for profile in chromium_profiles(root) {
        let db_path = profile.join("Network").join("Cookies");
        let db_path = if db_path.exists() {
            db_path
        } else {
            profile.join("Cookies")
        };
        if !db_path.exists() {
            continue;
        }

        let rows = query_chromium_cookies(&db_path)?;
        if let Some(header) = cookie_header_from_browser(&keys, rows) {
            return Some(header);
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn chromium_profiles(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();

    for name in ["Default", "Profile 1", "Profile 2", "Profile 3"] {
        let path = root.join(name);
        if path.is_dir() {
            out.push(path);
        }
    }

    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if path.is_dir()
                && (name == "Default" || name.starts_with("Profile "))
                && !out.iter().any(|existing| existing == &path)
            {
                out.push(path);
            }
        }
    }

    out
}

#[cfg(target_os = "windows")]
fn chromium_keys(local_state: &Path) -> Option<ChromiumKeys> {
    let txt = fs::read_to_string(local_state).ok()?;
    let json: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let encoded = json.pointer("/os_crypt/encrypted_key")?.as_str()?;
    let mut encrypted = BASE64.decode(encoded).ok()?;
    if encrypted.starts_with(b"DPAPI") {
        encrypted.drain(..5);
    }
    let legacy = crypt_unprotect(&encrypted)?;
    let app_bound = json
        .pointer("/os_crypt/app_bound_encrypted_key")
        .and_then(serde_json::Value::as_str)
        .and_then(chromium_app_bound_keys)
        .unwrap_or_default();

    Some(ChromiumKeys { legacy, app_bound })
}

#[cfg(target_os = "windows")]
fn chromium_app_bound_keys(encoded: &str) -> Option<Vec<Vec<u8>>> {
    let raw = BASE64.decode(encoded).ok()?;
    if !raw.starts_with(b"APPB") {
        return None;
    }

    let system_decrypted = crypt_unprotect_as_system(&raw[4..])?;
    let user_decrypted = crypt_unprotect(&system_decrypted)?;
    if user_decrypted.len() < 61 {
        return None;
    }

    let decrypted_key = &user_decrypted[user_decrypted.len() - 61..];
    let mut keys = vec![decrypted_key[decrypted_key.len() - 32..].to_vec()];

    let aes_key = BASE64
        .decode("sxxuJBrIRnKNqcH6xJNmUc/7lE0UOrgWJ2vMbaAoR4c=")
        .ok()?;
    let iv = &decrypted_key[1..13];
    let mut ciphertext = decrypted_key[13..45].to_vec();
    ciphertext.extend_from_slice(&decrypted_key[45..]);
    let cipher = Aes256Gcm::new_from_slice(&aes_key).ok()?;
    let plain = cipher
        .decrypt(GenericArray::from_slice(iv), ciphertext.as_slice())
        .ok()?;
    keys.push(plain);
    Some(keys)
}

#[cfg(target_os = "windows")]
fn query_chromium_cookies(db_path: &Path) -> Option<Vec<ChromiumCookieRow>> {
    let script = r#"
import base64, json, os, shutil, sqlite3, sys, tempfile
src = sys.argv[1]
with tempfile.TemporaryDirectory(prefix="rustplayer-cookies-") as tmp:
    dst = os.path.join(tmp, "Cookies")
    shutil.copy2(src, dst)
    for suffix in ("-wal", "-shm"):
        side = src + suffix
        if os.path.exists(side):
            shutil.copy2(side, dst + suffix)
    conn = sqlite3.connect(dst)
    cur = conn.cursor()
    cur.execute(
        "SELECT host_key, name, value, encrypted_value FROM cookies WHERE host_key LIKE ? OR host_key LIKE ? ORDER BY LENGTH(host_key) DESC",
        ("%youtube.com", "%music.youtube.com"),
    )
    rows = [
        {
            "domain": host_key or "",
            "name": name or "",
            "plain_value": value or "",
            "encrypted_value_b64": base64.b64encode(encrypted_value or b"").decode("ascii"),
        }
        for host_key, name, value, encrypted_value in cur.fetchall()
    ]
    print(json.dumps(rows))
"#;

    for (program, extra_args) in [("python", &[][..]), ("py", &["-3"][..])] {
        let mut cmd = Command::new(program);
        cmd.args(extra_args).arg("-c").arg(script).arg(db_path);
        let Ok(output) = cmd.output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let rows = serde_json::from_slice::<Vec<ChromiumCookieRow>>(&output.stdout).ok()?;
        if !rows.is_empty() {
            return Some(rows);
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn cookie_header_from_browser(
    keys: &ChromiumKeys,
    cookies: Vec<ChromiumCookieRow>,
) -> Option<String> {
    let mut best = BTreeMap::<String, (u8, String)>::new();

    for cookie in cookies {
        let name = cookie.name.trim();
        if name.is_empty() {
            continue;
        }
        let Some(value) = chromium_cookie_value(keys, &cookie) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }

        let rank = browser_cookie_rank(&cookie.domain);
        match best.get(name) {
            Some((current_rank, _)) if *current_rank > rank => continue,
            _ => {
                best.insert(name.to_string(), (rank, value.to_string()));
            }
        }
    }

    let header = best
        .into_iter()
        .map(|(name, (_, value))| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");

    if header.is_empty() {
        None
    } else {
        Some(header)
    }
}

#[cfg(target_os = "windows")]
fn chromium_cookie_value(keys: &ChromiumKeys, cookie: &ChromiumCookieRow) -> Option<String> {
    if !cookie.plain_value.trim().is_empty() {
        return Some(cookie.plain_value.clone());
    }

    let encrypted = BASE64.decode(&cookie.encrypted_value_b64).ok()?;
    if encrypted.starts_with(b"v20") {
        let mut candidates = keys.app_bound.clone();
        candidates.push(keys.legacy.clone());
        return decrypt_chromium_gcm(&encrypted, &candidates, true);
    }
    if encrypted.starts_with(b"v10") || encrypted.starts_with(b"v11") {
        return decrypt_chromium_gcm(&encrypted, std::slice::from_ref(&keys.legacy), false);
    }

    let decrypted = crypt_unprotect(&encrypted)?;
    String::from_utf8(decrypted).ok()
}

#[cfg(target_os = "windows")]
fn decrypt_chromium_gcm(
    encrypted: &[u8],
    candidate_keys: &[Vec<u8>],
    strip_prefix: bool,
) -> Option<String> {
    if encrypted.len() <= 15 {
        return None;
    }

    let nonce = Nonce::from_slice(&encrypted[3..15]);
    let ciphertext = &encrypted[15..];
    for key in candidate_keys {
        let cipher = Aes256Gcm::new_from_slice(key).ok()?;
        let decrypted = match cipher.decrypt(nonce, ciphertext) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let payload = if strip_prefix && decrypted.len() >= 32 {
            decrypted[32..].to_vec()
        } else {
            decrypted
        };
        if let Ok(text) = String::from_utf8(payload) {
            return Some(text);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn crypt_unprotect_as_system(data: &[u8]) -> Option<Vec<u8>> {
    let token = start_system_impersonation()?;
    let decrypted = crypt_unprotect(data);
    stop_system_impersonation(token);
    decrypted
}

#[cfg(target_os = "windows")]
fn crypt_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let mut in_blob = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    unsafe {
        if CryptUnprotectData(
            &mut in_blob,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
            &mut out_blob,
        ) == 0
        {
            return None;
        }

        let bytes = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        LocalFree(out_blob.pbData.cast());
        Some(bytes)
    }
}

#[cfg(target_os = "windows")]
fn start_system_impersonation() -> Option<HANDLE> {
    if !enable_debug_privilege() {
        return None;
    }
    let pid = system_process_pid()?;
    let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }

    let mut token = std::ptr::null_mut();
    let opened = unsafe { OpenProcessToken(process, TOKEN_DUPLICATE | TOKEN_QUERY, &mut token) };
    unsafe {
        CloseHandle(process);
    }
    if opened == 0 || token.is_null() {
        return None;
    }

    let mut duplicate = std::ptr::null_mut();
    let duplicated = unsafe { DuplicateToken(token, SecurityImpersonation, &mut duplicate) };
    unsafe {
        CloseHandle(token);
    }
    if duplicated == 0 || duplicate.is_null() {
        return None;
    }

    if unsafe { ImpersonateLoggedOnUser(duplicate) } == 0 {
        unsafe {
            CloseHandle(duplicate);
        }
        return None;
    }

    Some(duplicate)
}

#[cfg(target_os = "windows")]
fn stop_system_impersonation(token: HANDLE) {
    unsafe {
        CloseHandle(token);
        RevertToSelf();
    }
}

#[cfg(target_os = "windows")]
fn enable_debug_privilege() -> bool {
    let mut previous = 0;
    unsafe { RtlAdjustPrivilege(20, 1, 0, &mut previous) == 0 }
}

#[cfg(target_os = "windows")]
fn system_process_pid() -> Option<u32> {
    let mut needed = 0u32;
    let mut pids = vec![0u32; 1024];
    if unsafe { EnumProcesses(pids.as_mut_ptr(), (pids.len() * 4) as u32, &mut needed) } == 0 {
        return None;
    }
    pids.truncate((needed / 4) as usize);

    let mut fallback = None;
    for pid in pids {
        let Some(name) = process_name(pid) else {
            continue;
        };
        if name.eq_ignore_ascii_case("lsass.exe") {
            return Some(pid);
        }
        if name.eq_ignore_ascii_case("winlogon.exe") {
            fallback = Some(pid);
        }
    }
    fallback
}

#[cfg(target_os = "windows")]
fn process_name(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        return None;
    }

    let mut buffer = vec![0u16; 260];
    let len =
        unsafe { K32GetProcessImageFileNameW(handle, buffer.as_mut_ptr(), buffer.len() as u32) };
    unsafe {
        CloseHandle(handle);
    }
    if len == 0 {
        return None;
    }

    let path = OsString::from_wide(&buffer[..len as usize]);
    Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
}

#[cfg(target_os = "windows")]
fn browser_cookie_rank(domain: &str) -> u8 {
    if domain.contains("music.youtube.com") {
        3
    } else if domain.ends_with(".youtube.com") {
        2
    } else if domain.contains("youtube.com") {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_netscape_cookie_file() {
        let raw = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tVISITOR_INFO1_LIVE\tabc\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tdef\n";
        assert_eq!(
            parse_cookie_source(raw).as_deref(),
            Some("VISITOR_INFO1_LIVE=abc; SID=def")
        );
    }
}
