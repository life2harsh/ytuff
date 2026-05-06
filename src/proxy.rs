use reqwest::blocking::ClientBuilder;
use reqwest::Proxy;
use std::process::Command;

const PROXY_ENV_KEYS: [&str; 7] = [
    "RUSTPLAYER_PROXY",
    "ALL_PROXY",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "all_proxy",
    "https_proxy",
    "http_proxy",
];

pub fn configured_proxy_url() -> Option<String> {
    PROXY_ENV_KEYS.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

pub fn apply_reqwest_proxy(builder: ClientBuilder) -> ClientBuilder {
    let Some(proxy_url) = configured_proxy_url() else {
        return builder;
    };

    match Proxy::all(&proxy_url) {
        Ok(proxy) => builder.proxy(proxy),
        Err(_) => builder,
    }
}

pub fn apply_command_proxy(cmd: &mut Command) {
    let Some(proxy_url) = configured_proxy_url() else {
        return;
    };

    for key in [
        "RUSTPLAYER_PROXY",
        "ALL_PROXY",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "all_proxy",
        "https_proxy",
        "http_proxy",
    ] {
        cmd.env(key, &proxy_url);
    }
}

pub fn append_ytdlp_proxy_args(args: &mut Vec<String>) {
    let Some(proxy_url) = configured_proxy_url() else {
        return;
    };

    args.push("--proxy".to_string());
    args.push(proxy_url);
}
