//! Forge backends behind one trait. Each backend shells out to the forge's CLI
//! (`gh`, `glab`, `az repos`) with an argument list, never a shell string.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

const CONFIG_KEY: &str = "forge";
const GITHUB_HOST: &str = "github.com";
const GITLAB_HOST_MARK: &str = "gitlab";
const GITLAB_HOST: &str = "gitlab.com";
const AZURE_HOST: &str = "dev.azure.com";
const AZURE_HOST_SUFFIX: &str = ".visualstudio.com";
const GH: &str = "gh";
const GLAB: &str = "glab";
const AZ: &str = "az";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Github,
    Gitlab,
    Azure,
}

impl Kind {
    pub fn cli(self) -> &'static str {
        match self {
            Kind::Github => GH,
            Kind::Gitlab => GLAB,
            Kind::Azure => AZ,
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Kind::Github => "github",
            Kind::Gitlab => "gitlab",
            Kind::Azure => "azure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPr {
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
}

/// A pull request's description and the text of its comments, in forge order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrText {
    pub description: String,
    pub comments: Vec<String>,
}

pub trait Forge {
    /// The open pull request whose source branch is `branch`, if any.
    fn find_open(&self, branch: &str) -> Result<Option<Pr>>;
    fn open(&self, pr: &NewPr) -> Result<Pr>;
    fn read(&self, number: u64) -> Result<PrText>;
}

/// The host part of a git remote URL: `https://host/...`, `ssh://user@host/...`
/// or scp-like `user@host:path`.
pub fn remote_host(remote: &str) -> Option<String> {
    let authority = match remote.split_once("://") {
        Some((_, rest)) => rest.split(['/', '?']).next()?,
        None => remote.split_once(':')?.0,
    };
    let host = authority
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The forge a remote URL points at, or `None` when the host is unknown.
pub fn detect(remote: &str) -> Option<Kind> {
    let host = remote_host(remote)?;
    if host == GITHUB_HOST {
        Some(Kind::Github)
    } else if host == GITLAB_HOST || host.contains(GITLAB_HOST_MARK) {
        Some(Kind::Gitlab)
    } else if host == AZURE_HOST || host.ends_with(AZURE_HOST_SUFFIX) {
        Some(Kind::Azure)
    } else {
        None
    }
}

/// The forge for a repo: the configured `forge` key wins over the origin remote.
pub fn select(configured: Option<Kind>, remote: Option<&str>) -> Result<Kind> {
    if let Some(kind) = configured {
        return Ok(kind);
    }
    match remote {
        Some(url) => detect(url).ok_or_else(|| {
            anyhow!(
                "no forge for remote `{url}`; set `{CONFIG_KEY} = \"github\" | \"gitlab\" | \"azure\"` in the config"
            )
        }),
        None => Err(anyhow!(
            "no `origin` remote; set `{CONFIG_KEY} = \"github\" | \"gitlab\" | \"azure\"` in the config"
        )),
    }
}

/// The URL of the repo's `origin` remote, or `None` when it has none.
pub fn origin_url(repo: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .output()
        .context("running git remote get-url origin")?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

/// The backend for `kind`, running the CLI named by [`Kind::cli`] from `dir`.
pub fn backend(kind: Kind, dir: &Path) -> Box<dyn Forge> {
    backend_with(kind, PathBuf::from(kind.cli()), dir)
}

/// The backend for `kind` with an explicit program, for tests and tools that
/// keep the CLI outside `PATH`.
pub fn backend_with(kind: Kind, program: PathBuf, dir: &Path) -> Box<dyn Forge> {
    let cli = Cli {
        program,
        dir: dir.to_path_buf(),
    };
    match kind {
        Kind::Github => Box::new(GitHub(cli)),
        Kind::Gitlab => Box::new(GitLab(cli)),
        Kind::Azure => Box::new(Azure(cli)),
    }
}

struct Cli {
    program: PathBuf,
    dir: PathBuf,
}

impl Cli {
    fn run(&self, args: &[&str]) -> Result<String> {
        let output = Command::new(&self.program)
            .args(args)
            .current_dir(&self.dir)
            .output()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => anyhow!(
                    "`{}` is not installed or not on PATH",
                    self.program.display()
                ),
                _ => anyhow!("starting `{}`: {error}", self.program.display()),
            })?;
        if !output.status.success() {
            bail!(
                "`{} {}` failed: {}",
                self.program.display(),
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn number_from_url(url: &str) -> Result<u64> {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|tail| tail.parse().ok())
        .with_context(|| format!("no pull request number in `{url}`"))
}

fn last_line(text: &str) -> Result<&str> {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .context("the CLI printed no pull request URL")
}

/// `gh`. Flags from `gh pr list --help`, `gh pr create --help` and
/// `gh pr view --help` (gh 2.97.0), run against the help text, not a live repo.
struct GitHub(Cli);

#[derive(Deserialize)]
struct GhPr {
    number: u64,
    url: String,
}

#[derive(Deserialize)]
struct GhView {
    body: String,
    comments: Vec<GhComment>,
}

#[derive(Deserialize)]
struct GhComment {
    body: String,
}

impl Forge for GitHub {
    fn find_open(&self, branch: &str) -> Result<Option<Pr>> {
        let text = self.0.run(&[
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "open",
            "--json",
            "number,url",
        ])?;
        let prs: Vec<GhPr> = serde_json::from_str(&text).context("parsing gh pr list")?;
        Ok(prs.into_iter().next().map(|pr| Pr {
            number: pr.number,
            url: pr.url,
        }))
    }

    fn open(&self, pr: &NewPr) -> Result<Pr> {
        let text = self.0.run(&[
            "pr", "create", "--head", &pr.head, "--base", &pr.base, "--title", &pr.title, "--body",
            &pr.body,
        ])?;
        let url = last_line(&text)?.to_owned();
        Ok(Pr {
            number: number_from_url(&url)?,
            url,
        })
    }

    fn read(&self, number: u64) -> Result<PrText> {
        let number = number.to_string();
        let text = self
            .0
            .run(&["pr", "view", &number, "--json", "body,comments"])?;
        let view: GhView = serde_json::from_str(&text).context("parsing gh pr view")?;
        Ok(PrText {
            description: view.body,
            comments: view.comments.into_iter().map(|c| c.body).collect(),
        })
    }
}

/// `glab`. Flags from https://docs.gitlab.com/cli/mr/list/, /create/ and
/// /view/ (the view page lists `-c, --comments` and `-F, --output text|json`). The JSON field names (`iid`, `web_url`, `description`, `notes`)
/// and the `create` output format are not documented there and are unverified:
/// `glab` is not installed here, so nothing in this backend has run.
struct GitLab(Cli);

#[derive(Deserialize)]
struct GlabMr {
    iid: u64,
    web_url: String,
}

#[derive(Deserialize)]
struct GlabView {
    description: Option<String>,
    #[serde(default)]
    notes: Vec<GlabNote>,
}

#[derive(Deserialize)]
struct GlabNote {
    body: String,
}

impl Forge for GitLab {
    fn find_open(&self, branch: &str) -> Result<Option<Pr>> {
        let text = self
            .0
            .run(&["mr", "list", "--source-branch", branch, "--output", "json"])?;
        let mrs: Vec<GlabMr> = serde_json::from_str(&text).context("parsing glab mr list")?;
        Ok(mrs.into_iter().next().map(|mr| Pr {
            number: mr.iid,
            url: mr.web_url,
        }))
    }

    fn open(&self, pr: &NewPr) -> Result<Pr> {
        let text = self.0.run(&[
            "mr",
            "create",
            "--source-branch",
            &pr.head,
            "--target-branch",
            &pr.base,
            "--title",
            &pr.title,
            "--description",
            &pr.body,
            "--yes",
        ])?;
        let url = last_line(&text)?.to_owned();
        Ok(Pr {
            number: number_from_url(&url)?,
            url,
        })
    }

    fn read(&self, number: u64) -> Result<PrText> {
        let number = number.to_string();
        let text = self
            .0
            .run(&["mr", "view", &number, "--output", "json", "--comments"])?;
        let view: GlabView = serde_json::from_str(&text).context("parsing glab mr view")?;
        Ok(PrText {
            description: view.description.unwrap_or_default(),
            comments: view.notes.into_iter().map(|n| n.body).collect(),
        })
    }
}

/// `az repos pr`. Flags from https://learn.microsoft.com/en-us/cli/azure/repos/pr
/// (the group page; the per-command pages would not load here). Unverified:
/// `az` is not installed here. The `pullRequestId` and `_links.web.href` fields
/// are assumed. `az repos pr` has no comment listing, so `read` returns the
/// description only.
struct Azure(Cli);

#[derive(Deserialize)]
struct AzPr {
    #[serde(rename = "pullRequestId")]
    pull_request_id: u64,
    #[serde(rename = "_links")]
    links: AzLinks,
    description: Option<String>,
}

#[derive(Deserialize)]
struct AzLinks {
    web: AzHref,
}

#[derive(Deserialize)]
struct AzHref {
    href: String,
}

impl AzPr {
    fn pr(&self) -> Pr {
        Pr {
            number: self.pull_request_id,
            url: self.links.web.href.clone(),
        }
    }
}

impl Forge for Azure {
    fn find_open(&self, branch: &str) -> Result<Option<Pr>> {
        let text = self.0.run(&[
            "repos",
            "pr",
            "list",
            "--source-branch",
            branch,
            "--status",
            "active",
            "--output",
            "json",
        ])?;
        let prs: Vec<AzPr> = serde_json::from_str(&text).context("parsing az repos pr list")?;
        Ok(prs.first().map(AzPr::pr))
    }

    fn open(&self, pr: &NewPr) -> Result<Pr> {
        let text = self.0.run(&[
            "repos",
            "pr",
            "create",
            "--source-branch",
            &pr.head,
            "--target-branch",
            &pr.base,
            "--title",
            &pr.title,
            "--description",
            &pr.body,
            "--output",
            "json",
        ])?;
        let created: AzPr = serde_json::from_str(&text).context("parsing az repos pr create")?;
        Ok(created.pr())
    }

    fn read(&self, number: u64) -> Result<PrText> {
        let number = number.to_string();
        let text = self
            .0
            .run(&["repos", "pr", "show", "--id", &number, "--output", "json"])?;
        let shown: AzPr = serde_json::from_str(&text).context("parsing az repos pr show")?;
        Ok(PrText {
            description: shown.description.unwrap_or_default(),
            comments: Vec::new(),
        })
    }
}
