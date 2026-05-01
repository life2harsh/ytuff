use crate::appdata::AppPaths;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct AuthSession {
    pub cookie_header: String,
    pub auth_user: Option<String>,
}

const LOGIN_URL: &str =
    "https://accounts.google.com/ServiceLogin?continue=https%3A%2F%2Fmusic.youtube.com";
const MUSIC_URL: &str = "https://music.youtube.com";
const VIDEO_URL: &str = "https://www.youtube.com";
const TITLE_WAITING: &str =
    "RustPlayer YouTube Login - sign in, then close this window when you are done";
const TITLE_READY: &str = "RustPlayer YouTube Login - signed in, close this window to finish";

const AUTH_IPC_SCRIPT: &str = r#"
(() => {
  const emit = () => {
    try {
      const cfg =
        (window.yt && window.yt.config_) ||
        (window.ytcfg && window.ytcfg.data_) ||
        {};
      const params = new URLSearchParams(window.location.search);
      let authUser = null;
      if (params.has("authuser")) {
        authUser = params.get("authuser");
      } else if (cfg.SESSION_INDEX !== undefined && cfg.SESSION_INDEX !== null) {
        authUser = String(cfg.SESSION_INDEX);
      }
      window.ipc.postMessage(JSON.stringify({
        kind: "rustplayer_auth",
        url: window.location.href,
        authUser,
        title: document.title || null
      }));
    } catch (_) {}
  };
  emit();
  document.addEventListener("readystatechange", emit);
  window.addEventListener("load", emit);
  setInterval(emit, 1500);
})();
"#;

#[derive(Clone, Default)]
struct LoginCapture {
    auth_user: Option<String>,
    last_url: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct LoginIpcPayload {
    #[serde(rename = "authUser")]
    auth_user: Option<String>,
    url: Option<String>,
    title: Option<String>,
}

pub fn youtube_login_window(paths: &AppPaths) -> Result<AuthSession> {
    #[cfg(target_os = "windows")]
    {
        windows::youtube_login_window(paths)
    }

    #[cfg(target_os = "linux")]
    {
        linux::youtube_login_window(paths)
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = paths;
        Err(anyhow!(
            "Interactive YouTube login is currently only supported on Windows and Linux"
        ))
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use url::Url;
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::windows::EventLoopBuilderExtWindows;
    use winit::window::{Window, WindowId};
    use wry::{WebContext, WebView, WebViewBuilder};

    struct LoginWindowApp {
        profile_dir: PathBuf,
        capture: Arc<Mutex<LoginCapture>>,
        outcome: Arc<Mutex<Option<Result<AuthSession, String>>>>,
        window: Option<Window>,
        window_id: Option<WindowId>,
        webview: Option<WebView>,
        web_context: Option<WebContext>,
        last_status_poll: Instant,
        current_title: &'static str,
    }

    impl LoginWindowApp {
        fn new(
            profile_dir: PathBuf,
            capture: Arc<Mutex<LoginCapture>>,
            outcome: Arc<Mutex<Option<Result<AuthSession, String>>>>,
        ) -> Self {
            Self {
                profile_dir,
                capture,
                outcome,
                window: None,
                window_id: None,
                webview: None,
                web_context: None,
                last_status_poll: Instant::now(),
                current_title: TITLE_WAITING,
            }
        }

        fn set_outcome(&self, result: Result<AuthSession>) {
            *self.outcome.lock().unwrap() = Some(result.map_err(|err| err.to_string()));
        }

        fn finish(&mut self, event_loop: &ActiveEventLoop) {
            let result = self
                .webview
                .as_ref()
                .ok_or_else(|| anyhow!("The YouTube login window did not initialize correctly"))
                .and_then(collect_auth_session)
                .map(|mut session| {
                    if session.auth_user.is_none() {
                        let capture = self.capture.lock().unwrap().clone();
                        session.auth_user = capture
                            .auth_user
                            .or_else(|| capture.last_url.as_deref().and_then(auth_user_from_url))
                            .or_else(|| Some("0".to_string()));
                    }
                    session
                });

            self.set_outcome(result);
            event_loop.exit();
        }
    }

    impl ApplicationHandler for LoginWindowApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }

            let attrs = Window::default_attributes()
                .with_title(TITLE_WAITING)
                .with_resizable(true)
                .with_inner_size(LogicalSize::new(980.0, 760.0));

            let window = match event_loop.create_window(attrs) {
                Ok(window) => window,
                Err(err) => {
                    self.set_outcome(Err(anyhow!(
                        "Could not create the YouTube login window: {}",
                        err
                    )));
                    event_loop.exit();
                    return;
                }
            };

            let mut context = WebContext::new(Some(self.profile_dir.clone()));
            let capture = Arc::clone(&self.capture);
            let builder = WebViewBuilder::new_with_web_context(&mut context)
                .with_url(LOGIN_URL)
                .with_initialization_script(AUTH_IPC_SCRIPT)
                .with_ipc_handler(move |req| {
                    let Ok(payload) = serde_json::from_str::<LoginIpcPayload>(req.body()) else {
                        return;
                    };
                    let mut capture = capture.lock().unwrap();
                    if let Some(auth_user) = payload
                        .auth_user
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                    {
                        capture.auth_user = Some(auth_user);
                    }
                    if let Some(url) = payload
                        .url
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                    {
                        capture.last_url = Some(url);
                    }
                    if let Some(title) = payload
                        .title
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                    {
                        capture.title = Some(title);
                    }
                });

            let webview = match builder.build(&window) {
                Ok(webview) => webview,
                Err(err) => {
                    self.set_outcome(Err(anyhow!(
                        "Could not start the YouTube login webview: {}",
                        err
                    )));
                    event_loop.exit();
                    return;
                }
            };

            self.window_id = Some(window.id());
            self.window = Some(window);
            self.webview = Some(webview);
            self.web_context = Some(context);
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            window_id: WindowId,
            event: WindowEvent,
        ) {
            if Some(window_id) != self.window_id {
                return;
            }

            if matches!(event, WindowEvent::CloseRequested) {
                self.finish(event_loop);
            }
        }

        fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
            if self.last_status_poll.elapsed() < Duration::from_millis(750) {
                return;
            }
            self.last_status_poll = Instant::now();

            let ready = self
                .webview
                .as_ref()
                .and_then(|webview| collect_youtube_cookie_header(webview).ok())
                .is_some_and(|header| header_contains_auth_cookie(&header));
            let next_title = if ready { TITLE_READY } else { TITLE_WAITING };

            if next_title != self.current_title {
                if let Some(window) = self.window.as_ref() {
                    window.set_title(next_title);
                }
                self.current_title = next_title;
            }
        }
    }

    pub fn youtube_login_window(paths: &AppPaths) -> Result<AuthSession> {
        paths.ensure()?;
        let profile_dir = login_profile_dir(paths);
        if profile_dir.exists() {
            let _ = fs::remove_dir_all(&profile_dir);
        }
        fs::create_dir_all(&profile_dir)
            .with_context(|| format!("Could not create {}", profile_dir.display()))?;

        let capture = Arc::new(Mutex::new(LoginCapture::default()));
        let outcome = Arc::new(Mutex::new(None));

        let mut builder = EventLoop::<()>::with_user_event();
        builder.with_any_thread(true);
        let event_loop = builder
            .build()
            .context("Could not start the YouTube login event loop")?;

        let mut app = LoginWindowApp::new(profile_dir.clone(), capture, Arc::clone(&outcome));
        event_loop
            .run_app(&mut app)
            .context("The YouTube login window terminated unexpectedly")?;

        let result =
            outcome.lock().unwrap().take().ok_or_else(|| {
                anyhow!("The YouTube login window closed without returning a result")
            })?;

        thread::sleep(Duration::from_millis(250));
        let _ = fs::remove_dir_all(&profile_dir);

        result.map_err(|err| anyhow!(err))
    }

    fn login_profile_dir(paths: &AppPaths) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        paths
            .cache_dir
            .join(format!("youtube-login-webview-{stamp}"))
    }

    fn collect_auth_session(webview: &WebView) -> Result<AuthSession> {
        let cookie_header = collect_youtube_cookie_header(webview)?;
        if !header_contains_auth_cookie(&cookie_header) {
            return Err(anyhow!(
                "No YouTube Music login was captured. Sign in, let music.youtube.com finish loading, then close the window."
            ));
        }

        Ok(AuthSession {
            cookie_header,
            auth_user: None,
        })
    }

    fn collect_youtube_cookie_header(webview: &WebView) -> Result<String> {
        let mut cookies = BTreeMap::<String, String>::new();

        for url in [MUSIC_URL, VIDEO_URL] {
            for cookie in webview
                .cookies_for_url(url)
                .with_context(|| format!("Could not read webview cookies for {}", url))?
            {
                let name = cookie.name().trim();
                let value = cookie.value().trim();
                if !name.is_empty() && !value.is_empty() {
                    cookies.insert(name.to_string(), value.to_string());
                }
            }
        }

        let header = cookies
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");

        if header.trim().is_empty() {
            Err(anyhow!(
                "No YouTube Music cookies were captured from the login window"
            ))
        } else {
            Ok(header)
        }
    }

    fn header_contains_auth_cookie(header: &str) -> bool {
        header.contains("SAPISID=") || header.contains("__Secure-3PAPISID=")
    }

    fn auth_user_from_url(url: &str) -> Option<String> {
        Url::parse(url)
            .ok()?
            .query_pairs()
            .find(|(key, _)| key == "authuser")
            .map(|(_, value)| value.to_string())
            .filter(|value| !value.trim().is_empty())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use gtk::prelude::*;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use url::Url;
    use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtUnix};

    pub fn youtube_login_window(paths: &AppPaths) -> Result<AuthSession> {
        paths.ensure()?;

        let _locale_override = LinuxLocaleOverride::maybe_force_utf8();
        init_linux_webview_runtime()?;

        let profile_dir = login_profile_dir(paths);
        if profile_dir.exists() {
            let _ = fs::remove_dir_all(&profile_dir);
        }
        fs::create_dir_all(&profile_dir)
            .with_context(|| format!("Could not create {}", profile_dir.display()))?;

        let capture = Arc::new(Mutex::new(LoginCapture::default()));
        let outcome = Arc::new(Mutex::new(None));

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title(TITLE_WAITING);
        window.set_resizable(true);
        window.set_default_size(980, 760);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&vbox);

        let mut context = WebContext::new(Some(profile_dir.clone()));
        let capture_for_ipc = Arc::clone(&capture);
        let builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(LOGIN_URL)
            .with_initialization_script(AUTH_IPC_SCRIPT)
            .with_ipc_handler(move |req| {
                let Ok(payload) = serde_json::from_str::<LoginIpcPayload>(req.body()) else {
                    return;
                };
                let mut capture = capture_for_ipc.lock().unwrap();
                if let Some(auth_user) = payload
                    .auth_user
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                {
                    capture.auth_user = Some(auth_user);
                }
                if let Some(url) = payload
                    .url
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                {
                    capture.last_url = Some(url);
                }
                if let Some(title) = payload
                    .title
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                {
                    capture.title = Some(title);
                }
            });

        let webview = Rc::new(
            builder
                .build_gtk(&vbox)
                .map_err(|err| anyhow!("Could not start the YouTube login webview: {}", err))?,
        );

        let ready_webview = Rc::clone(&webview);
        let ready_window = window.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(750), move || {
            let ready = collect_youtube_cookie_header(ready_webview.as_ref())
                .ok()
                .is_some_and(|header| header_contains_auth_cookie(&header));
            ready_window.set_title(if ready { TITLE_READY } else { TITLE_WAITING });
            gtk::glib::ControlFlow::Continue
        });

        let close_capture = Arc::clone(&capture);
        let close_outcome = Arc::clone(&outcome);
        let close_webview = Rc::clone(&webview);
        window.connect_delete_event(move |_, _| {
            let result = collect_auth_session(close_webview.as_ref()).map(|mut session| {
                if session.auth_user.is_none() {
                    let capture = close_capture.lock().unwrap().clone();
                    session.auth_user = capture
                        .auth_user
                        .or_else(|| capture.last_url.as_deref().and_then(auth_user_from_url))
                        .or_else(|| Some("0".to_string()));
                }
                session
            });

            *close_outcome.lock().unwrap() = Some(result.map_err(|err| err.to_string()));
            if gtk::main_level() > 0 {
                gtk::main_quit();
            }
            gtk::glib::Propagation::Proceed
        });

        window.connect_destroy(|_| {
            if gtk::main_level() > 0 {
                gtk::main_quit();
            }
        });

        window.show_all();
        window.present();
        gtk::main();

        let result =
            outcome.lock().unwrap().take().ok_or_else(|| {
                anyhow!("The YouTube login window closed without returning a result")
            })?;

        thread::sleep(Duration::from_millis(250));
        let _ = fs::remove_dir_all(&profile_dir);

        result.map_err(|err| anyhow!(err))
    }

    fn login_profile_dir(paths: &AppPaths) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        paths
            .cache_dir
            .join(format!("youtube-login-webview-{stamp}"))
    }

    fn collect_auth_session(webview: &WebView) -> Result<AuthSession> {
        let cookie_header = collect_youtube_cookie_header(webview)?;
        if !header_contains_auth_cookie(&cookie_header) {
            return Err(anyhow!(
                "No YouTube Music login was captured. Sign in, let music.youtube.com finish loading, then close the window."
            ));
        }

        Ok(AuthSession {
            cookie_header,
            auth_user: None,
        })
    }

    fn collect_youtube_cookie_header(webview: &WebView) -> Result<String> {
        let mut cookies = BTreeMap::<String, String>::new();

        for url in [MUSIC_URL, VIDEO_URL] {
            for cookie in webview
                .cookies_for_url(url)
                .with_context(|| format!("Could not read webview cookies for {}", url))?
            {
                let name = cookie.name().trim();
                let value = cookie.value().trim();
                if !name.is_empty() && !value.is_empty() {
                    cookies.insert(name.to_string(), value.to_string());
                }
            }
        }

        let header = cookies
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");

        if header.trim().is_empty() {
            Err(anyhow!(
                "No YouTube Music cookies were captured from the login window"
            ))
        } else {
            Ok(header)
        }
    }

    fn header_contains_auth_cookie(header: &str) -> bool {
        header.contains("SAPISID=") || header.contains("__Secure-3PAPISID=")
    }

    fn auth_user_from_url(url: &str) -> Option<String> {
        Url::parse(url)
            .ok()?
            .query_pairs()
            .find(|(key, _)| key == "authuser")
            .map(|(_, value)| value.to_string())
            .filter(|value| !value.trim().is_empty())
    }

    fn init_linux_webview_runtime() -> Result<()> {
        if gtk::is_initialized_main_thread() {
            return Ok(());
        }

        gtk::init().map_err(|err| {
            anyhow!(
                "Could not initialize GTK for the YouTube login window: {}. Start RustPlayer from a graphical Linux session and try again.",
                err
            )
        })
    }

    struct LinuxLocaleOverride {
        lang: Option<OsString>,
        lc_all: Option<OsString>,
        lc_ctype: Option<OsString>,
    }

    impl LinuxLocaleOverride {
        fn maybe_force_utf8() -> Option<Self> {
            if linux_locale_is_utf8() {
                return None;
            }

            let original = Self {
                lang: std::env::var_os("LANG"),
                lc_all: std::env::var_os("LC_ALL"),
                lc_ctype: std::env::var_os("LC_CTYPE"),
            };

            for key in ["LANG", "LC_ALL", "LC_CTYPE"] {
                std::env::set_var(key, "C.UTF-8");
            }

            Some(original)
        }
    }

    impl Drop for LinuxLocaleOverride {
        fn drop(&mut self) {
            restore_env_var("LANG", self.lang.as_ref());
            restore_env_var("LC_ALL", self.lc_all.as_ref());
            restore_env_var("LC_CTYPE", self.lc_ctype.as_ref());
        }
    }

    fn linux_locale_is_utf8() -> bool {
        ["LC_ALL", "LC_CTYPE", "LANG"]
            .into_iter()
            .find_map(|key| std::env::var_os(key))
            .map(|value| {
                let value = value.to_string_lossy().to_ascii_lowercase();
                value.contains("utf-8") || value.contains("utf8")
            })
            .unwrap_or(false)
    }

    fn restore_env_var(name: &str, value: Option<&OsString>) {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }
}
