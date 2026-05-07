use crate::core::track::{Acc, Track};
use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use rand::RngCore;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::{form_urlencoded, Url};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Ql {
    Low,
    Med,
    High,
}

impl Ql {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "low" => Self::Low,
            "medium" | "med" => Self::Med,
            _ => Self::High,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Med => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScState {
    pub ready: bool,
    pub user: bool,
    pub name: Option<String>,
    pub msg: Option<String>,
    pub ql: String,
}

#[derive(Clone, Debug)]
pub struct ScStream {
    pub url: String,
    pub tag: String,
}

#[derive(Clone)]
pub struct SoundCloudClient {
    http: Client,
    cfg: Option<Cfg>,
    tok: TokFile,
    ql: Ql,
}

#[derive(Clone)]
struct Cfg {
    id: String,
    sec: String,
    red: String,
    path: PathBuf,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TokFile {
    app: Option<Tok>,
    user: Option<Tok>,
    me: Option<Me>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Tok {
    acc: String,
    ref_tok: Option<String>,
    exp: u64,
    scope: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Me {
    name: String,
    link: Option<String>,
    img: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct TokRsp {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    scope: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiUser {
    username: Option<String>,
    permalink_url: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiTrack {
    id: u64,
    title: String,
    duration: Option<u64>,
    permalink_url: Option<String>,
    artwork_url: Option<String>,
    stream_url: Option<String>,
    access: Option<String>,
    metadata_artist: Option<String>,
    user: Option<ApiUser>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiKind {
    kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiColl {
    collection: Vec<ApiTrack>,
}

#[derive(Clone, Debug, Deserialize)]
struct StreamRsp {
    hls_aac_160_url: Option<String>,
    hls_aac_96_url: Option<String>,
    preview_mp3_128_url: Option<String>,
    http_mp3_128_url: Option<String>,
}

impl SoundCloudClient {
    pub fn new(ql: Ql) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(25))
            .user_agent(concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| Client::new());
        let cfg = load_cfg();
        let tok = cfg
            .as_ref()
            .and_then(|cfg| fs::read_to_string(&cfg.path).ok())
            .and_then(|txt| serde_json::from_str::<TokFile>(&txt).ok())
            .unwrap_or_default();
        Self { http, cfg, tok, ql }
    }

    pub fn state(&self) -> ScState {
        let mut st = ScState {
            ready: self.cfg.is_some(),
            user: self.tok.user.as_ref().is_some_and(is_live),
            name: self.tok.me.as_ref().map(|me| me.name.clone()),
            msg: None,
            ql: self.ql.as_str().to_string(),
        };
        if self.cfg.is_none() {
            st.msg = Some("set SOUNDCLOUD_CLIENT_ID and SOUNDCLOUD_CLIENT_SECRET".to_string());
        } else if !st.user {
            st.msg = Some("app auth ready, press l to sign in".to_string());
        }
        st
    }

    pub fn login(&mut self) -> Result<ScState> {
        let cfg = self
            .cfg
            .clone()
            .context("SoundCloud credentials are not set")?;
        let red = Url::parse(&cfg.red).context("Invalid redirect URI")?;
        let host = red
            .host_str()
            .ok_or_else(|| anyhow!("Redirect URI must include a host"))?
            .to_string();
        let port = red
            .port_or_known_default()
            .ok_or_else(|| anyhow!("Redirect URI must include a port"))?;
        let bind = format!("{host}:{port}");
        let lis = TcpListener::bind(&bind).with_context(|| format!("Could not bind {bind}"))?;
        lis.set_nonblocking(true)?;

        let ver = rand_b64(32);
        let state = rand_b64(24);
        let chal = pkce_chal(&ver);
        let mut auth = Url::parse("https://secure.soundcloud.com/authorize")?;
        auth.query_pairs_mut()
            .append_pair("client_id", &cfg.id)
            .append_pair("redirect_uri", &cfg.red)
            .append_pair("response_type", "code")
            .append_pair("code_challenge", &chal)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);

        webbrowser::open(auth.as_str()).context("Could not open browser")?;

        let end = now() + 180;
        let mut code = None;
        let mut got_state = None;
        while now() < end {
            match lis.accept() {
                Ok((mut st, _)) => {
                    let mut buf = [0u8; 8192];
                    let n = st.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let line = req.lines().next().unwrap_or_default();
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let full = format!("{}://{}{}", red.scheme(), bind, path);
                    let url = Url::parse(&full).ok();
                    let mut ok = false;
                    if let Some(url) = url {
                        if url.path() == red.path() {
                            code = url
                                .query_pairs()
                                .find(|(k, _)| k == "code")
                                .map(|(_, v)| v.into_owned());
                            got_state = url
                                .query_pairs()
                                .find(|(k, _)| k == "state")
                                .map(|(_, v)| v.into_owned());
                            ok = code.is_some();
                        }
                    }
                    let body = if ok {
                        "<html><body><h2>YTuff is connected.</h2><p>You can return to the terminal.</p></body></html>"
                    } else {
                        "<html><body><h2>YTuff login failed.</h2><p>Close this tab and try again.</p></body></html>"
                    };
                    let rsp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = st.write_all(rsp.as_bytes());
                    let _ = st.flush();
                    if ok {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e.into()),
            }
        }

        let code = code.ok_or_else(|| anyhow!("Timed out waiting for SoundCloud login"))?;
        if got_state.as_deref() != Some(state.as_str()) {
            return Err(anyhow!("SoundCloud state check failed"));
        }

        let rsp = self
            .http
            .post("https://secure.soundcloud.com/oauth/token")
            .header(ACCEPT, "application/json; charset=utf-8")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", cfg.id.as_str()),
                ("client_secret", cfg.sec.as_str()),
                ("redirect_uri", cfg.red.as_str()),
                ("code_verifier", ver.as_str()),
                ("code", code.as_str()),
            ])
            .send()?
            .error_for_status()?
            .json::<TokRsp>()?;

        self.tok.user = Some(tok_of(rsp));
        self.tok.me = Some(self.me()?);
        self.save()?;
        Ok(self.state())
    }

    pub fn logout(&mut self) -> Result<ScState> {
        if let Some(tok) = self.tok.user.as_ref() {
            let _ = self
                .http
                .post("https://secure.soundcloud.com/sign-out")
                .header(ACCEPT, "application/json; charset=utf-8")
                .json(&serde_json::json!({ "access_token": tok.acc }))
                .send();
        }
        self.tok.user = None;
        self.tok.me = None;
        self.save()?;
        Ok(self.state())
    }

    pub fn search(&mut self, q: &str, lim: usize) -> Result<Vec<Track>> {
        let q = q.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        if is_soundcloud_url(q) {
            return Ok(self.resolve(q)?.into_iter().collect());
        }
        let tok = self.ensure_tok(false)?;
        let rsp = self
            .http
            .get("https://api.soundcloud.com/tracks")
            .header(ACCEPT, "application/json; charset=utf-8")
            .header(AUTHORIZATION, format!("OAuth {}", tok))
            .query(&[
                ("q", q),
                ("access", "playable,preview"),
                ("linked_partitioning", "true"),
                ("limit", &lim.to_string()),
            ])
            .send()?
            .error_for_status()?
            .json::<ApiColl>()?;
        Ok(rsp.collection.into_iter().map(map_track).collect())
    }

    pub fn resolve(&mut self, url: &str) -> Result<Option<Track>> {
        let tok = self.ensure_tok(false)?;
        let rsp = self
            .http
            .get("https://api.soundcloud.com/resolve")
            .header(ACCEPT, "application/json; charset=utf-8")
            .header(AUTHORIZATION, format!("OAuth {}", tok))
            .query(&[("url", url)])
            .send()?
            .error_for_status()?;

        let txt = rsp.text()?;
        let kind = serde_json::from_str::<ApiKind>(&txt)?;
        match kind.kind.as_deref() {
            Some("track") => Ok(Some(map_track(serde_json::from_str::<ApiTrack>(&txt)?))),
            Some("playlist") => Err(anyhow!("Playlists are not supported yet")),
            Some("user") => Err(anyhow!("Resolved to a user, not a track")),
            _ => Ok(None),
        }
    }

    pub fn stream(&mut self, tr: &Track) -> Result<ScStream> {
        if !tr.is_sc() {
            return Err(anyhow!("Track is not from SoundCloud"));
        }
        if tr.acc == Some(Acc::Block) {
            return Err(anyhow!("This SoundCloud track is blocked"));
        }

        let tok = self.ensure_tok(false)?;
        let id = tr.id.trim_start_matches("sc:");
        let base = tr
            .strm
            .clone()
            .unwrap_or_else(|| format!("https://api.soundcloud.com/tracks/{id}/streams"));

        let mut rsp = self
            .http
            .get(&base)
            .header(ACCEPT, "application/json; charset=utf-8")
            .header(AUTHORIZATION, format!("OAuth {}", tok))
            .send();

        if rsp
            .as_ref()
            .ok()
            .and_then(|v| v.status().as_u16().checked_sub(404))
            .is_some()
        {
            let alt = if base.ends_with("/streams") {
                format!("https://api.soundcloud.com/tracks/{id}/stream")
            } else {
                format!("https://api.soundcloud.com/tracks/{id}/streams")
            };
            rsp = self
                .http
                .get(&alt)
                .header(ACCEPT, "application/json; charset=utf-8")
                .header(AUTHORIZATION, format!("OAuth {}", tok))
                .send();
        }

        let rsp = rsp?.error_for_status()?.json::<StreamRsp>()?;
        pick_stream(&rsp, self.ql)
    }

    pub fn art(&self, url: &str) -> Result<Vec<u8>> {
        Ok(self
            .http
            .get(url)
            .send()?
            .error_for_status()?
            .bytes()?
            .to_vec())
    }
    fn me(&mut self) -> Result<Me> {
        let tok = self.ensure_tok(true)?;
        let rsp = self
            .http
            .get("https://api.soundcloud.com/me")
            .header(ACCEPT, "application/json; charset=utf-8")
            .header(AUTHORIZATION, format!("OAuth {}", tok))
            .send()?
            .error_for_status()?
            .json::<ApiUser>()?;
        Ok(Me {
            name: rsp.username.unwrap_or_else(|| "SoundCloud".to_string()),
            link: rsp.permalink_url,
            img: rsp.avatar_url.map(up_art),
        })
    }

    fn ensure_tok(&mut self, user: bool) -> Result<String> {
        if user {
            if self.tok.user.as_ref().is_some_and(is_live) {
                return Ok(self.tok.user.as_ref().unwrap().acc.clone());
            }
            if self.tok.user.is_some() {
                self.ref_user()?;
                if self.tok.user.as_ref().is_some_and(is_live) {
                    return Ok(self.tok.user.as_ref().unwrap().acc.clone());
                }
            }
        }

        if self.tok.app.as_ref().is_some_and(is_live) {
            return Ok(self.tok.app.as_ref().unwrap().acc.clone());
        }
        if self.tok.app.is_some() && self.ref_app().is_ok() {
            if self.tok.app.as_ref().is_some_and(is_live) {
                return Ok(self.tok.app.as_ref().unwrap().acc.clone());
            }
        }
        self.app_tok()?;
        self.tok
            .app
            .as_ref()
            .map(|tok| tok.acc.clone())
            .ok_or_else(|| anyhow!("Could not get app token"))
    }

    fn app_tok(&mut self) -> Result<()> {
        let cfg = self
            .cfg
            .clone()
            .context("SoundCloud credentials are not set")?;
        let auth = STANDARD.encode(format!("{}:{}", cfg.id, cfg.sec));
        let rsp = self
            .http
            .post("https://secure.soundcloud.com/oauth/token")
            .header(ACCEPT, "application/json; charset=utf-8")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(AUTHORIZATION, format!("Basic {}", auth))
            .form(&[("grant_type", "client_credentials")])
            .send()?
            .error_for_status()?
            .json::<TokRsp>()?;
        self.tok.app = Some(tok_of(rsp));
        self.save()
    }

    fn ref_app(&mut self) -> Result<()> {
        let cfg = self
            .cfg
            .clone()
            .context("SoundCloud credentials are not set")?;
        let ref_tok = self
            .tok
            .app
            .as_ref()
            .and_then(|tok| tok.ref_tok.clone())
            .context("No refresh token")?;
        let rsp = self
            .http
            .post("https://secure.soundcloud.com/oauth/token")
            .header(ACCEPT, "application/json; charset=utf-8")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", cfg.id.as_str()),
                ("client_secret", cfg.sec.as_str()),
                ("refresh_token", ref_tok.as_str()),
            ])
            .send()?
            .error_for_status()?
            .json::<TokRsp>()?;
        self.tok.app = Some(tok_of(rsp));
        self.save()
    }

    fn ref_user(&mut self) -> Result<()> {
        let cfg = self
            .cfg
            .clone()
            .context("SoundCloud credentials are not set")?;
        let ref_tok = self
            .tok
            .user
            .as_ref()
            .and_then(|tok| tok.ref_tok.clone())
            .context("No refresh token")?;
        let rsp = self
            .http
            .post("https://secure.soundcloud.com/oauth/token")
            .header(ACCEPT, "application/json; charset=utf-8")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", cfg.id.as_str()),
                ("client_secret", cfg.sec.as_str()),
                ("refresh_token", ref_tok.as_str()),
            ])
            .send()?
            .error_for_status()?
            .json::<TokRsp>()?;
        self.tok.user = Some(tok_of(rsp));
        self.tok.me = Some(self.me()?);
        self.save()
    }

    fn save(&self) -> Result<()> {
        let Some(cfg) = self.cfg.as_ref() else {
            return Ok(());
        };
        if let Some(dir) = cfg.path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&cfg.path, serde_json::to_vec_pretty(&self.tok)?)?;
        Ok(())
    }
}

fn load_cfg() -> Option<Cfg> {
    let id = env::var("SOUNDCLOUD_CLIENT_ID").ok()?;
    let sec = env::var("SOUNDCLOUD_CLIENT_SECRET").ok()?;
    let red = env::var("SOUNDCLOUD_REDIRECT_URI")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8974/sc/cb".to_string());
    let mut dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.push("ytuff");
    dir.push("soundcloud.json");
    Some(Cfg {
        id,
        sec,
        red,
        path: dir,
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn is_live(tok: &Tok) -> bool {
    tok.exp > now() + 15
}

fn tok_of(rsp: TokRsp) -> Tok {
    Tok {
        acc: rsp.access_token,
        ref_tok: rsp.refresh_token,
        exp: now() + rsp.expires_in.saturating_sub(30),
        scope: rsp.scope,
    }
}

fn rand_b64(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn pkce_chal(ver: &str) -> String {
    let mut sha = Sha256::new();
    sha.update(ver.as_bytes());
    URL_SAFE_NO_PAD.encode(sha.finalize())
}

fn map_track(tr: ApiTrack) -> Track {
    let artist = tr
        .metadata_artist
        .clone()
        .or_else(|| tr.user.as_ref().and_then(|u| u.username.clone()));
    let user = tr.user.as_ref().and_then(|u| u.username.clone());
    let art = tr
        .artwork_url
        .or_else(|| tr.user.as_ref().and_then(|u| u.avatar_url.clone()))
        .map(up_art);
    let acc = match tr.access.as_deref() {
        Some("playable") => Some(Acc::Play),
        Some("preview") => Some(Acc::Prev),
        Some("blocked") => Some(Acc::Block),
        _ => None,
    };

    Track::new_sc(
        format!("sc:{}", tr.id),
        tr.title,
        artist,
        user,
        tr.duration.map(|v| v / 1000),
        tr.permalink_url,
        art,
        tr.stream_url,
        acc,
    )
}

fn up_art(mut url: String) -> String {
    for from in ["-large.", "-t300x300.", "-crop.", "-t200x200."] {
        if url.contains(from) {
            url = url.replace(from, "-t500x500.");
        }
    }
    url
}

fn pick_stream(rsp: &StreamRsp, ql: Ql) -> Result<ScStream> {
    let pick = match ql {
        Ql::High => rsp
            .hls_aac_160_url
            .clone()
            .map(|v| (v, "aac160".to_string()))
            .or_else(|| rsp.hls_aac_96_url.clone().map(|v| (v, "aac96".to_string())))
            .or_else(|| {
                rsp.preview_mp3_128_url
                    .clone()
                    .map(|v| (v, "preview".to_string()))
            })
            .or_else(|| rsp.http_mp3_128_url.clone().map(|v| (v, "mp3".to_string()))),
        Ql::Med => rsp
            .hls_aac_96_url
            .clone()
            .map(|v| (v, "aac96".to_string()))
            .or_else(|| {
                rsp.hls_aac_160_url
                    .clone()
                    .map(|v| (v, "aac160".to_string()))
            })
            .or_else(|| {
                rsp.preview_mp3_128_url
                    .clone()
                    .map(|v| (v, "preview".to_string()))
            })
            .or_else(|| rsp.http_mp3_128_url.clone().map(|v| (v, "mp3".to_string()))),
        Ql::Low => rsp
            .preview_mp3_128_url
            .clone()
            .map(|v| (v, "preview".to_string()))
            .or_else(|| rsp.hls_aac_96_url.clone().map(|v| (v, "aac96".to_string())))
            .or_else(|| {
                rsp.hls_aac_160_url
                    .clone()
                    .map(|v| (v, "aac160".to_string()))
            })
            .or_else(|| rsp.http_mp3_128_url.clone().map(|v| (v, "mp3".to_string()))),
    };

    let (url, tag) = pick.ok_or_else(|| anyhow!("No playable SoundCloud stream was returned"))?;
    Ok(ScStream { url, tag })
}

pub fn is_soundcloud_url(url: &str) -> bool {
    if let Ok(parsed) = Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            return host.contains("soundcloud.com") || host.contains("snd.sc");
        }
    }
    false
}

pub fn build_auth_link(url: &str) -> String {
    if url.starts_with("https://soundcloud.com") || url.starts_with("http://soundcloud.com") {
        url.to_string()
    } else {
        let mut out = String::from("https://soundcloud.com?");
        out.push_str(
            &form_urlencoded::Serializer::new(String::new())
                .append_pair("to", url)
                .finish(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sc_url() {
        assert!(is_soundcloud_url("https://soundcloud.com/a/b"));
        assert!(is_soundcloud_url("https://snd.sc/abc"));
        assert!(!is_soundcloud_url("https://example.com"));
    }

    #[test]
    fn art_size() {
        let url = up_art("https://i1.sndcdn.com/artworks-000-large.jpg".to_string());
        assert!(url.contains("t500x500"));
    }

    #[test]
    fn stream_pref() {
        let rsp = StreamRsp {
            hls_aac_160_url: Some("a".into()),
            hls_aac_96_url: Some("b".into()),
            preview_mp3_128_url: Some("c".into()),
            http_mp3_128_url: None,
        };
        assert_eq!(pick_stream(&rsp, Ql::High).unwrap().url, "a");
        assert_eq!(pick_stream(&rsp, Ql::Med).unwrap().url, "b");
        assert_eq!(pick_stream(&rsp, Ql::Low).unwrap().url, "c");
    }
}
