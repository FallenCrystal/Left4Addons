use crate::CoreError;
use url::Url;

pub fn validate_external_url(url: &str) -> Result<String, CoreError> {
    let invalid = || CoreError::InvalidInput("仅支持 Steam 商店和社区链接".into());
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) || url.contains('\\') {
        return Err(invalid());
    }

    if let Some(id) = url.strip_prefix("steam://url/CommunityFilePage/") {
        if id.bytes().all(|b| b.is_ascii_digit()) && id.bytes().any(|b| b != b'0') {
            return Ok(url.to_owned());
        }
        return Err(invalid());
    }

    let parsed = Url::parse(url).map_err(|_| invalid())?;
    if url
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
        && parsed.scheme() == "https"
        && matches!(
            parsed.host_str(),
            Some("steamcommunity.com" | "store.steampowered.com")
        )
        && parsed.username().is_empty()
        && parsed.password().is_none()
    {
        Ok(parsed.into())
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_steam_store_community_and_workshop_links() {
        for url in [
            "https://steamcommunity.com/sharedfiles/filedetails/?id=123",
            "https://store.steampowered.com/app/550/",
            "steam://url/CommunityFilePage/123",
            "steam://url/CommunityFilePage/000123",
        ] {
            assert_eq!(validate_external_url(url).unwrap(), url);
        }
        for url in [
            "",
            "http://steamcommunity.com/",
            "https://steamcommunity.com.example.com/",
            "https://store.steampowered.com@example.com/",
            "https://example.com@steamcommunity.com/",
            "https:steamcommunity.com/",
            "https://steamcommunity.com\\path",
            "https://steamcommunity.com/\n",
            "file:///tmp/local",
            "steam://run/550",
            "steam://url/CommunityFilePage/",
            "steam://url/CommunityFilePage/00",
            "steam://url/CommunityFilePage/-1",
            "steam://url/CommunityFilePage/１２３",
            "steam://url/CommunityFilePage/123?x=y",
            "steam://url/CommunityFilePage/123/",
            "steam://url/CommunityFilePage/123 ",
            "steam://url/CommunityFilePage/12\t3",
        ] {
            assert!(
                matches!(validate_external_url(url), Err(CoreError::InvalidInput(_))),
                "accepted {url:?}"
            );
        }
    }
}
