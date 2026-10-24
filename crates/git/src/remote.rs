//! Remotes, and the hosted repository (GitHub, GitHub Enterprise, …) they
//! point at.

use std::path::Path;

use anyhow::Result;

use crate::run::git;

/// A repository on a hosting service, parsed from a remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedRepo {
    pub remote: String,
    /// `github.com`, or an enterprise host such as `github.example.net`.
    pub host: String,
    /// The owner (user or organization); may hold `/` for nested groups.
    pub owner: String,
    pub name: String,
}

/// `(name, URL)` for each remote, as configured: before `insteadOf`
/// rewrites, so a mirror or SSH rewrite still names the hosting repo.
pub fn remotes(root: &Path) -> Result<Vec<(String, String)>> {
    // No remotes is not an error (git exits 1 when nothing matches).
    let out = git(root, &["config", "--get-regexp", r"^remote\..*\.url$"]).unwrap_or_default();
    let mut list: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let Some((key, url)) = line.split_once(' ') else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|key| key.strip_suffix(".url"))
        else {
            continue;
        };
        if !list.iter().any(|(known, _)| known == name) {
            list.push((name.to_string(), url.trim().to_string()));
        }
    }
    Ok(list)
}

/// The hosted repo behind `origin` (else `upstream`, else the first remote).
pub fn hosted_repo(root: &Path) -> Option<HostedRepo> {
    let remotes = remotes(root).ok()?;
    let pick = ["origin", "upstream"]
        .iter()
        .find_map(|wanted| remotes.iter().find(|(name, _)| name == wanted))
        .or(remotes.first())?;
    let (host, owner, name) = parse_url(&pick.1)?;
    Some(HostedRepo {
        remote: pick.0.clone(),
        host,
        owner,
        name,
    })
}

/// `(host, owner, repo)` from an HTTPS, SSH or scp-style git URL.
pub fn parse_url(url: &str) -> Option<(String, String, String)> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, rest)| rest) {
        // https://[user@]host[:port]/owner/repo, ssh://git@host[:port]/owner/repo
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?;
        (host.to_string(), path)
    } else {
        // git@host:owner/repo
        let (authority, path) = url.split_once(':')?;
        let host = authority.rsplit('@').next()?;
        (host.to_string(), path)
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, name) = path.rsplit_once('/')?;
    if host.is_empty() || owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((
        host.to_ascii_lowercase(),
        owner.to_string(),
        name.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    #[test]
    fn parses_remote_urls() {
        let expect = |url: &str, host: &str, owner: &str, name: &str| {
            assert_eq!(
                parse_url(url),
                Some((host.to_string(), owner.to_string(), name.to_string())),
                "{url}"
            );
        };
        expect(
            "https://github.com/acme/app.git",
            "github.com",
            "acme",
            "app",
        );
        expect("git@github.com:acme/app.git", "github.com", "acme", "app");
        expect(
            "ssh://git@github.sec.example.net:2222/team/app",
            "github.sec.example.net",
            "team",
            "app",
        );
        expect(
            "https://user@GitHub.Example.com/org/sub/app/",
            "github.example.com",
            "org/sub",
            "app",
        );
        assert_eq!(parse_url("not a url"), None);
        assert_eq!(parse_url("https://github.com/only"), None);
    }

    #[test]
    fn finds_the_origin_repo() {
        let root = temp_repo("remote");
        git(
            &root,
            &["remote", "add", "upstream", "https://github.com/up/app.git"],
        )
        .unwrap();
        git(
            &root,
            &["remote", "add", "origin", "git@github.com:me/app.git"],
        )
        .unwrap();
        let repo = hosted_repo(&root).unwrap();
        assert_eq!(
            (repo.remote.as_str(), repo.owner.as_str()),
            ("origin", "me")
        );
    }
}
