use url::Url;

pub(crate) const BMCLAPI_BASE_URL: &str = "https://bmclapi2.bangbang93.com";

fn join(namespace: &str, path: &str) -> String {
    format!(
        "{}/{}/{}",
        BMCLAPI_BASE_URL.trim_end_matches('/'),
        namespace.trim_matches('/'),
        path.replace('\\', "/").trim_start_matches('/')
    )
}

fn replace_host(source_url: &str) -> Result<String, String> {
    let parsed = Url::parse(source_url)
        .map_err(|error| format!("Invalid resource URL: {} ({})", source_url, error))?;
    let path = parsed.path().trim_start_matches('/');
    if path.is_empty() {
        return Err(format!("Resource URL is missing a path: {}", source_url));
    }

    let mut rewritten = format!("{}/{}", BMCLAPI_BASE_URL.trim_end_matches('/'), path);
    if let Some(query) = parsed.query() {
        rewritten.push('?');
        rewritten.push_str(query);
    }
    Ok(rewritten)
}

pub(crate) fn version_client_url(source_url: &str) -> Result<String, String> {
    // PCL's LauncherOrMeta rule only replaces the piston-data host. Client
    // URLs are content-addressed and custom version IDs are not valid routes.
    replace_host(source_url)
}

pub(crate) fn library_urls(path: &str) -> Vec<String> {
    // PCL tries both BMCLAPI compatibility routes for Maven artifacts.
    vec![join("maven", path), join("libraries", path)]
}

pub(crate) fn asset_object_url(path: &str) -> String {
    join("assets", path)
}

pub(crate) fn asset_index_url(source_url: &str) -> Result<String, String> {
    let parsed = Url::parse(source_url)
        .map_err(|error| format!("资源索引 URL 无效: {} ({})", source_url, error))?;
    let path = parsed.path().trim_start_matches('/');
    if path.is_empty() {
        return Err(format!("资源索引 URL 缺少路径: {}", source_url));
    }

    // BMCLAPI metadata endpoints replace only the host. They do not use the
    // `/assets` object prefix used by resources.download.minecraft.net.
    Ok(format!(
        "{}/{}",
        BMCLAPI_BASE_URL.trim_end_matches('/'),
        path
    ))
}

pub(crate) fn is_bmclapi_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "bmclapi2.bangbang93.com")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kinds_use_distinct_prefixes() {
        assert_eq!(
            version_client_url("https://piston-data.mojang.com/v1/objects/client-hash/client.jar")
                .unwrap(),
            "https://bmclapi2.bangbang93.com/v1/objects/client-hash/client.jar"
        );
        assert_eq!(
            library_urls("com/example/demo/1.0/demo-1.0.jar"),
            vec![
                "https://bmclapi2.bangbang93.com/maven/com/example/demo/1.0/demo-1.0.jar",
                "https://bmclapi2.bangbang93.com/libraries/com/example/demo/1.0/demo-1.0.jar",
            ]
        );
        assert_eq!(
            asset_object_url("ab/abcdef"),
            "https://bmclapi2.bangbang93.com/assets/ab/abcdef"
        );
    }

    #[test]
    fn asset_index_preserves_metadata_path_without_assets_prefix() {
        let url = asset_index_url("https://piston-meta.mojang.com/v1/packages/hash-value/17.json")
            .expect("asset index URL should parse");
        assert_eq!(
            url,
            "https://bmclapi2.bangbang93.com/v1/packages/hash-value/17.json"
        );
    }

    #[test]
    fn bmclapi_detection_is_host_based() {
        assert!(is_bmclapi_url(
            "https://bmclapi2.bangbang93.com/maven/a.jar"
        ));
        assert!(!is_bmclapi_url(
            "https://example.com/?next=bmclapi2.bangbang93.com"
        ));
    }
}
