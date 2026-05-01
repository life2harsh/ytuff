use crate::core::track::Track;
use crate::sources::soundcloud::SoundCloudClient;
use anyhow::{anyhow, Context, Result};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadFormat {
    M4a,
    Mp3,
}

impl DownloadFormat {
    pub fn ext(self) -> &'static str {
        match self {
            Self::M4a => "m4a",
            Self::Mp3 => "mp3",
        }
    }
}

pub fn download_track(
    track: &Track,
    client: &mut SoundCloudClient,
    format: DownloadFormat,
    output_dir: &Path,
) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Could not create {}", output_dir.display()))?;

    let base_name = sanitize_filename(&format!("{} - {}", track.who(), track.title));
    let final_path = output_dir.join(format!("{}.{}", base_name, format.ext()));

    if let Some(path) = track.path.as_ref() {
        return export_local_track(path, &final_path, format);
    }

    let stream = client.stream(track)?;
    let bytes = client.download_stream(&stream.url)?;

    match format {
        DownloadFormat::M4a => {
            fs::write(&final_path, bytes)
                .with_context(|| format!("Could not write {}", final_path.display()))?;
            Ok(final_path)
        }
        DownloadFormat::Mp3 => {
            let temp_source = temp_source_path("m4a");
            fs::write(&temp_source, bytes)
                .with_context(|| format!("Could not write {}", temp_source.display()))?;
            let res =
                transcode_with_ffmpeg(&temp_source, &final_path, "libmp3lame", &["-q:a", "2"]);
            let _ = fs::remove_file(&temp_source);
            res.map(|_| final_path)
        }
    }
}

fn export_local_track(source: &Path, final_path: &Path, format: DownloadFormat) -> Result<PathBuf> {
    let current_ext = source
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if current_ext == format.ext() {
        fs::copy(source, final_path).with_context(|| {
            format!(
                "Could not copy {} to {}",
                source.display(),
                final_path.display()
            )
        })?;
        return Ok(final_path.to_path_buf());
    }

    let codec = match format {
        DownloadFormat::M4a => "aac",
        DownloadFormat::Mp3 => "libmp3lame",
    };
    let extra = match format {
        DownloadFormat::M4a => vec!["-b:a", "192k"],
        DownloadFormat::Mp3 => vec!["-q:a", "2"],
    };
    transcode_with_ffmpeg(source, final_path, codec, &extra)?;
    Ok(final_path.to_path_buf())
}

fn transcode_with_ffmpeg(input: &Path, output: &Path, codec: &str, extra: &[&str]) -> Result<()> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-vn")
        .arg("-c:a")
        .arg(codec);
    for arg in extra {
        cmd.arg(arg);
    }
    let status = cmd
        .arg(output)
        .status()
        .context("Could not launch ffmpeg")?;
    if !status.success() {
        return Err(anyhow!("ffmpeg exited with status {}", status));
    }
    Ok(())
}

fn temp_source_path(ext: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    path.push(format!("rustplayer-dl-{stamp}.{ext}"));
    path
}

fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.' | '(' | ')') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out.trim().trim_matches('.').to_string()
}
