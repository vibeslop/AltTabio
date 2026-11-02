//! HTTPS and untrusted GitHub metadata. No process is launched during checks or downloads.
use super::native::sha256;
use alttabio::update::{Asset, Release, Version};
use alttabio::update_signature::{MAX_ARCHIVE_BYTES, MAX_SIGNATURE_BYTES};
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

mod transport;
use transport::get;

const API: &str = "https://api.github.com/repos/vibeslop/AltTabio/releases";
const MAX_METADATA: u64 = 2 * 1024 * 1024;

pub(super) fn releases() -> Result<Vec<Release>, String> {
    parse_releases(&json(&format!("{API}?per_page=30"))?)
}

fn parse_releases(value: &Value) -> Result<Vec<Release>, String> {
    let list = value
        .as_array()
        .ok_or("GitHub did not return a release list")?;
    Ok(list
        .iter()
        .filter_map(|item| {
            let tag = item.get("tag_name")?.as_str()?;
            let version = Version::parse(tag)?;
            if tag != format!("v{version}") {
                return None;
            }
            let name = format!("AltTabio-{version}-windows-x64.zip");
            let url = asset_url(version, &name);
            let assets = item
                .get("assets")?
                .as_array()?
                .iter()
                .filter(|asset| {
                    asset["name"].as_str() == Some(&name)
                        && asset["browser_download_url"].as_str() == Some(&url)
                })
                .map(|_| Asset {
                    name: name.clone(),
                    url: url.clone(),
                })
                .collect();
            Some(Release {
                tag: tag.to_owned(),
                draft: item.get("draft")?.as_bool()?,
                prerelease: item.get("prerelease")?.as_bool()?,
                assets,
            })
        })
        .collect())
}

fn asset_url(version: Version, name: &str) -> String {
    format!("https://github.com/vibeslop/AltTabio/releases/download/v{version}/{name}")
}

fn asset<'a>(
    release: &'a Value,
    version: Version,
    name: &str,
    limit: u64,
) -> Result<(&'a str, u64), String> {
    let assets = release["assets"]
        .as_array()
        .ok_or("Missing release assets")?;
    let mut matches = assets
        .iter()
        .filter(|item| item["name"].as_str() == Some(name));
    let asset = matches.next().ok_or("Missing update asset")?;
    if matches.next().is_some() {
        return Err("The release has duplicate update assets".to_owned());
    }
    let url = asset["browser_download_url"]
        .as_str()
        .ok_or("Missing update URL")?;
    let size = asset["size"].as_u64().ok_or("Missing update size")?;
    if url != asset_url(version, name) || size == 0 || size > limit {
        return Err("The release has an invalid update URL or size".to_owned());
    }
    Ok((url, size))
}

pub(super) fn stage(version: Version, work: &Path) -> Result<(), String> {
    stage_with(
        version,
        work,
        &json(&format!("{API}/tags/v{version}"))?,
        |url, limit, output| get(url, limit, Duration::from_mins(10), output),
    )
}

fn stage_with(
    version: Version,
    work: &Path,
    release: &Value,
    fetch: impl Fn(&str, u64, &mut dyn Write) -> Result<(), String>,
) -> Result<(), String> {
    if release["tag_name"].as_str() != Some(&format!("v{version}"))
        || release["draft"].as_bool() != Some(false)
        || release["prerelease"].as_bool() != Some(false)
    {
        return Err("This release is no longer a published stable release".to_owned());
    }
    let name = format!("AltTabio-{version}-windows-x64.zip");
    let (url, size) = asset(release, version, &name, MAX_ARCHIVE_BYTES)?;
    let digest = release["assets"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["name"].as_str() == Some(&name))
        })
        .and_then(|asset| asset["digest"].as_str())
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|hash| valid_hash(hash))
        .ok_or("The release has no verifiable Windows archive")?;
    let (signature_url, signature_size) = asset(
        release,
        version,
        &format!("{name}.minisig"),
        MAX_SIGNATURE_BYTES,
    )?;
    for (url, size, name) in [
        (url, size, "download.zip"),
        (signature_url, signature_size, "download.zip.minisig"),
    ] {
        let path = work.join(name);
        let mut output = std::fs::File::create_new(&path).map_err(|error| error.to_string())?;
        fetch(url, size, &mut output)?;
        output.sync_all().map_err(|error| error.to_string())?;
        if output.metadata().map_err(|error| error.to_string())?.len() != size {
            return Err("The downloaded update has the wrong size".to_owned());
        }
    }
    if !sha256(&work.join("download.zip"))?.eq_ignore_ascii_case(digest) {
        return Err("The downloaded archive failed its SHA-256 check".to_owned());
    }
    Ok(())
}

pub(super) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn json(url: &str) -> Result<Value, String> {
    let mut bytes = Vec::new();
    get(url, MAX_METADATA, Duration::from_secs(30), &mut bytes)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Unreadable GitHub release metadata: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_metadata_rejects_untrusted_urls_flags_sizes_and_duplicate_assets() {
        let version = Version::parse("9.0.0").unwrap_or_else(|| panic!("version"));
        let work = super::super::WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        let archive = b"archive";
        let path = work.path.join("fixture");
        std::fs::write(&path, archive).unwrap_or_else(|error| panic!("{error}"));
        let hash = sha256(&path).unwrap_or_else(|error| panic!("{error}"));
        let name = "AltTabio-9.0.0-windows-x64.zip";
        let release = json!({"tag_name":"v9.0.0", "draft":false, "prerelease":false, "assets":[
            {"name":name,"browser_download_url":asset_url(version,name),"size":archive.len(),"digest":format!("sha256:{hash}")},
            {"name":format!("{name}.minisig"),"browser_download_url":asset_url(version,&format!("{name}.minisig")),"size":3}
        ]});
        let parsed =
            parse_releases(&json!([release.clone()])).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(parsed[0].assets.len(), 1);
        assert!(parse_releases(&release).is_err());
        for pointer in [
            "/draft",
            "/prerelease",
            "/assets/0/browser_download_url",
            "/assets/0/size",
            "/assets/0/digest",
            "/assets/1/size",
        ] {
            let mut bad = release.clone();
            *bad.pointer_mut(pointer)
                .unwrap_or_else(|| panic!("pointer")) = json!("invalid");
            assert!(
                stage_with(version, &work.path, &bad, |_, _, _| panic!(
                    "invalid metadata must not download"
                ))
                .is_err()
            );
        }
        let mut duplicate = release.clone();
        duplicate["assets"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("assets"))
            .push(release["assets"][0].clone());
        assert!(
            stage_with(version, &work.path, &duplicate, |_, _, _| panic!(
                "duplicate asset"
            ))
            .is_err()
        );
        stage_with(version, &work.path, &release, |url, _, output| {
            output
                .write_all(if url.ends_with(".minisig") {
                    b"sig"
                } else {
                    archive
                })
                .map_err(|error| error.to_string())
        })
        .unwrap_or_else(|error| panic!("{error}"));
        for url in [
            "http://github.com/anything",
            "https://evil.example/anything",
        ] {
            assert!(get(url, 100, Duration::from_secs(1), &mut Vec::new()).is_err());
        }
    }

    #[test]
    #[ignore = "contacts the public GitHub API; run explicitly for network verification"]
    fn native_https_reads_live_github_metadata() {
        assert!(
            !releases()
                .unwrap_or_else(|error| panic!("{error}"))
                .is_empty()
        );
    }
}
