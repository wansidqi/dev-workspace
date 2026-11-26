//! The optional GitHub integration (CLAUDE.md "Git client", phase 5): sign in
//! with the OAuth device flow, list and create pull requests, read CI check
//! status. Plain blocking HTTP (`ureq`), meant to run on a background task.
//!
//! The access token is passed in by the caller (which keeps it in the OS
//! keychain); this crate never stores it and never puts it in an error.

use ureq::serde_json::{self, Value};

mod issues;
pub use issues::*;

const API: &str = "https://api.github.com";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const USER_AGENT: &str = "devspace";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GithubError {
    /// The token was refused (expired, revoked or missing a scope).
    #[error("GitHub didn't accept the sign-in. Sign in again.")]
    Unauthorized,
    /// GitHub answered with an error the user can read.
    #[error("{0}")]
    Api(String),
    /// Couldn't reach GitHub at all.
    #[error("couldn't reach GitHub: {0}")]
    Network(String),
    /// An owner, repo or branch name that isn't safe to put in a URL.
    #[error("{0}")]
    Invalid(String),
}

fn network(e: &ureq::Error) -> GithubError {
    // Transport errors can mention URLs; none of ours carry the token.
    GithubError::Network(e.to_string())
}

/// Reads an API error response into something readable.
fn api_error(status: u16, body: &str) -> GithubError {
    if status == 401 {
        return GithubError::Unauthorized;
    }
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("GitHub returned an error")
        .to_owned();
    // Validation errors carry the useful part in `errors[].message`.
    let detail = parsed
        .as_ref()
        .and_then(|v| v.get("errors"))
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(|e| {
                    e.get("message")
                        .and_then(Value::as_str)
                        .or_else(|| e.as_str())
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|d| !d.is_empty());
    GithubError::Api(match detail {
        Some(d) => format!("{message}: {d}"),
        None => format!("{message} (HTTP {status})"),
    })
}

fn finish(result: Result<ureq::Response, ureq::Error>) -> Result<Value, GithubError> {
    match result {
        Ok(response) => response
            .into_json::<Value>()
            .map_err(|e| GithubError::Network(e.to_string())),
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            Err(api_error(status, &body))
        }
        Err(e) => Err(network(&e)),
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
}

fn get(token: &str, url: &str) -> Result<Value, GithubError> {
    finish(
        agent()
            .get(url)
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("Authorization", &format!("Bearer {token}"))
            .call(),
    )
}

// ---- names ----------------------------------------------------------------

/// An owner or repository name as GitHub allows them.
fn is_name(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part.len() <= 100
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Percent-encodes one URL path segment (a branch name has slashes in it).
fn encode_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A GitHub repository, from a remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

/// `owner/name` from a github.com remote URL in any of its forms
/// (`https://github.com/o/r.git`, `git@github.com:o/r.git`,
/// `ssh://git@github.com/o/r`). `None` for other hosts.
pub fn parse_github_remote(url: &str) -> Option<RepoRef> {
    let url = url.trim();
    let rest = if let Some(r) = url.strip_prefix("git@github.com:") {
        r
    } else {
        let after_scheme = url.split_once("://").map(|(_, r)| r)?;
        let after_user = after_scheme
            .split_once('@')
            .filter(|(user, _)| !user.contains('/'))
            .map_or(after_scheme, |(_, r)| r);
        let (host, path) = after_user.split_once('/')?;
        let host = host.split(':').next()?;
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return None;
        }
        path
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, name) = rest.split_once('/')?;
    (is_name(owner) && is_name(name) && !name.contains('/')).then(|| RepoRef {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

fn repo_path(repo: &RepoRef) -> Result<String, GithubError> {
    if is_name(&repo.owner) && is_name(&repo.name) {
        Ok(format!("{API}/repos/{}/{}", repo.owner, repo.name))
    } else {
        Err(GithubError::Invalid("unusable repository name".into()))
    }
}

// ---- device flow -----------------------------------------------------------

/// What the user has to do to authorize the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    /// Shown to the user, typed at `verification_uri`.
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

fn form_post(url: &str, fields: &[(&str, &str)]) -> Result<Value, GithubError> {
    finish(
        agent()
            .post(url)
            .set("Accept", "application/json")
            .send_form(fields),
    )
}

/// Step 1 of the device flow. `client_id` is the OAuth app's public id (the
/// app must have "Enable Device Flow" ticked).
pub fn start_device_flow(client_id: &str) -> Result<DeviceCode, GithubError> {
    if client_id.trim().is_empty() {
        return Err(GithubError::Invalid(
            "enter the OAuth app's client id first".into(),
        ));
    }
    let v = form_post(
        DEVICE_CODE_URL,
        &[("client_id", client_id.trim()), ("scope", "repo read:user")],
    )?;
    parse_device_code(&v)
}

fn parse_device_code(v: &Value) -> Result<DeviceCode, GithubError> {
    if let Some(error) = v.get("error").and_then(Value::as_str) {
        let description = v
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or(error);
        return Err(GithubError::Api(description.to_owned()));
    }
    let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
    let number = |key: &str| v.get(key).and_then(Value::as_u64);
    Ok(DeviceCode {
        device_code: text("device_code")
            .ok_or_else(|| GithubError::Api("no device code".into()))?,
        user_code: text("user_code").ok_or_else(|| GithubError::Api("no user code".into()))?,
        verification_uri: text("verification_uri")
            .ok_or_else(|| GithubError::Api("no verification address".into()))?,
        expires_in: number("expires_in").unwrap_or(900),
        interval: number("interval").unwrap_or(5).max(1),
    })
}

/// One poll of step 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// Signed in: the access token.
    Token(String),
    /// The user hasn't approved yet; poll again after the interval.
    Pending,
    /// Poll less often: add this many seconds to the interval.
    SlowDown(u64),
    Denied,
    Expired,
}

pub fn poll_device_flow(client_id: &str, device_code: &str) -> Result<PollOutcome, GithubError> {
    let v = form_post(
        ACCESS_TOKEN_URL,
        &[
            ("client_id", client_id.trim()),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
    )?;
    parse_poll(&v)
}

fn parse_poll(v: &Value) -> Result<PollOutcome, GithubError> {
    if let Some(token) = v.get("access_token").and_then(Value::as_str) {
        return Ok(PollOutcome::Token(token.to_owned()));
    }
    match v.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        Some("slow_down") => Ok(PollOutcome::SlowDown(
            v.get("interval").and_then(Value::as_u64).unwrap_or(5),
        )),
        Some("access_denied") => Ok(PollOutcome::Denied),
        Some("expired_token") => Ok(PollOutcome::Expired),
        Some(other) => Err(GithubError::Api(
            v.get("error_description")
                .and_then(Value::as_str)
                .unwrap_or(other)
                .to_owned(),
        )),
        None => Err(GithubError::Api("unexpected answer from GitHub".into())),
    }
}

/// The signed-in account's login name.
pub fn current_user(token: &str) -> Result<String, GithubError> {
    let v = get(token, &format!("{API}/user"))?;
    v.get("login")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| GithubError::Api("GitHub didn't say who you are".into()))
}

// ---- pull requests ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub head_branch: String,
    pub base_branch: String,
    pub draft: bool,
    pub url: String,
    pub updated_at: String,
}

fn parse_pull(v: &Value) -> Option<PullRequest> {
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(PullRequest {
        number: v.get("number")?.as_u64()?,
        title: text(v, "title")?,
        author: v
            .get("user")
            .and_then(|u| text(u, "login"))
            .unwrap_or_default(),
        head_branch: v.get("head").and_then(|h| text(h, "ref"))?,
        base_branch: v.get("base").and_then(|b| text(b, "ref"))?,
        draft: v.get("draft").and_then(Value::as_bool).unwrap_or(false),
        url: text(v, "html_url")?,
        updated_at: text(v, "updated_at").unwrap_or_default(),
    })
}

/// Open pull requests of the repo, most recently updated first.
pub fn list_pull_requests(token: &str, repo: &RepoRef) -> Result<Vec<PullRequest>, GithubError> {
    let url = format!(
        "{}/pulls?state=open&sort=updated&direction=desc&per_page=30",
        repo_path(repo)?
    );
    let v = get(token, &url)?;
    Ok(v.as_array()
        .map(|list| list.iter().filter_map(parse_pull).collect())
        .unwrap_or_default())
}

/// Open pull requests whose head is `branch` (to show the branch's PR).
pub fn pull_requests_for_branch(
    token: &str,
    repo: &RepoRef,
    branch: &str,
) -> Result<Vec<PullRequest>, GithubError> {
    let head = format!("{}:{branch}", repo.owner);
    let url = format!(
        "{}/pulls?state=open&head={}",
        repo_path(repo)?,
        encode_segment(&head)
    );
    let v = get(token, &url)?;
    Ok(v.as_array()
        .map(|list| list.iter().filter_map(parse_pull).collect())
        .unwrap_or_default())
}

/// What to open a pull request with.
#[derive(Debug, Clone)]
pub struct NewPullRequest<'a> {
    pub title: &'a str,
    pub body: &'a str,
    /// The branch with the changes (must already be pushed).
    pub head: &'a str,
    pub base: &'a str,
    pub draft: bool,
}

pub fn create_pull_request(
    token: &str,
    repo: &RepoRef,
    request: &NewPullRequest<'_>,
) -> Result<PullRequest, GithubError> {
    if request.title.trim().is_empty() {
        return Err(GithubError::Invalid(
            "write a title for the pull request".into(),
        ));
    }
    let url = format!("{}/pulls", repo_path(repo)?);
    let body = serde_json::json!({
        "title": request.title.trim(),
        "body": request.body,
        "head": request.head,
        "base": request.base,
        "draft": request.draft,
    });
    let v = finish(
        agent()
            .post(&url)
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("Authorization", &format!("Bearer {token}"))
            .send_json(body),
    )?;
    parse_pull(&v).ok_or_else(|| GithubError::Api("GitHub's answer wasn't a pull request".into()))
}

// ---- checks ----------------------------------------------------------------

/// How a commit's CI is doing overall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// No checks ran.
    None,
    Pending,
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub state: CheckState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checks {
    pub state: CheckState,
    pub runs: Vec<CheckRun>,
}

fn run_state(run: &Value) -> CheckState {
    match run.get("status").and_then(Value::as_str) {
        Some("completed") => match run.get("conclusion").and_then(Value::as_str) {
            Some("success" | "neutral" | "skipped") => CheckState::Success,
            Some(_) => CheckState::Failure,
            None => CheckState::Pending,
        },
        _ => CheckState::Pending,
    }
}

/// Summarises the check-runs response: any failure fails, otherwise any
/// unfinished run is pending, otherwise success.
fn parse_checks(v: &Value) -> Checks {
    let runs: Vec<CheckRun> = v
        .get("check_runs")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|r| CheckRun {
                    name: r
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("check")
                        .to_owned(),
                    state: run_state(r),
                })
                .collect()
        })
        .unwrap_or_default();
    let state = if runs.is_empty() {
        CheckState::None
    } else if runs.iter().any(|r| r.state == CheckState::Failure) {
        CheckState::Failure
    } else if runs.iter().any(|r| r.state == CheckState::Pending) {
        CheckState::Pending
    } else {
        CheckState::Success
    };
    Checks { state, runs }
}

/// CI check runs for a branch or commit.
pub fn check_status(token: &str, repo: &RepoRef, git_ref: &str) -> Result<Checks, GithubError> {
    if git_ref.is_empty() || git_ref.starts_with('-') {
        return Err(GithubError::Invalid("unusable branch name".into()));
    }
    let url = format!(
        "{}/commits/{}/check-runs?per_page=50",
        repo_path(repo)?,
        encode_segment(git_ref)
    );
    Ok(parse_checks(&get(token, &url)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo(owner: &str, name: &str) -> Option<RepoRef> {
        Some(RepoRef {
            owner: owner.into(),
            name: name.into(),
        })
    }

    #[test]
    fn github_remotes_in_every_form_are_recognised() {
        for url in [
            "https://github.com/acme/tool.git",
            "https://github.com/acme/tool",
            "https://github.com/acme/tool/",
            "https://user:token@github.com/acme/tool.git",
            "git@github.com:acme/tool.git",
            "ssh://git@github.com/acme/tool.git",
            "ssh://git@github.com:22/acme/tool",
        ] {
            assert_eq!(parse_github_remote(url), repo("acme", "tool"), "{url}");
        }
        assert_eq!(
            parse_github_remote("https://gitlab.com/acme/tool.git"),
            None
        );
        assert_eq!(parse_github_remote("https://github.com/acme"), None);
        assert_eq!(parse_github_remote("C:\\src\\tool"), None);
        assert_eq!(parse_github_remote("https://github.com/a b/c"), None);
        assert_eq!(parse_github_remote("https://github.com/../c"), None);
    }

    #[test]
    fn urls_never_carry_unsafe_names() {
        assert!(repo_path(&RepoRef {
            owner: "a/b".into(),
            name: "c".into()
        })
        .is_err());
        assert_eq!(encode_segment("feature/a b"), "feature%2Fa%20b");
        assert_eq!(encode_segment("acme:main"), "acme%3Amain");
    }

    #[test]
    fn device_code_and_poll_answers_parse() {
        let code = parse_device_code(&json!({
            "device_code": "dc", "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900, "interval": 5
        }))
        .unwrap();
        assert_eq!((code.user_code.as_str(), code.interval), ("ABCD-1234", 5));
        assert!(
            parse_device_code(&json!({"error": "incorrect_client_credentials",
            "error_description": "The client_id is not valid"}))
            .unwrap_err()
            .to_string()
            .contains("client_id")
        );

        assert_eq!(
            parse_poll(&json!({"access_token": "tok", "token_type": "bearer"})),
            Ok(PollOutcome::Token("tok".into()))
        );
        assert_eq!(
            parse_poll(&json!({"error": "authorization_pending"})),
            Ok(PollOutcome::Pending)
        );
        assert_eq!(
            parse_poll(&json!({"error": "slow_down", "interval": 10})),
            Ok(PollOutcome::SlowDown(10))
        );
        assert_eq!(
            parse_poll(&json!({"error": "access_denied"})),
            Ok(PollOutcome::Denied)
        );
        assert_eq!(
            parse_poll(&json!({"error": "expired_token"})),
            Ok(PollOutcome::Expired)
        );
        assert!(parse_poll(&json!({})).is_err());
    }

    #[test]
    fn pull_requests_parse() {
        let pr = parse_pull(&json!({
            "number": 7, "title": "Fix it", "draft": true,
            "html_url": "https://github.com/a/b/pull/7",
            "updated_at": "2026-01-02T03:04:05Z",
            "user": {"login": "pat"},
            "head": {"ref": "fix"}, "base": {"ref": "main"}
        }))
        .unwrap();
        assert_eq!(
            (pr.number, pr.draft, pr.head_branch.as_str()),
            (7, true, "fix")
        );
        assert_eq!(pr.author, "pat");
        assert!(parse_pull(&json!({"number": 1})).is_none());
    }

    #[test]
    fn check_runs_summarise_failure_over_pending_over_success() {
        let runs = |items: Vec<(&str, Option<&str>)>| {
            parse_checks(&json!({"check_runs": items.into_iter().map(|(s, c)| json!({
                "name": "ci", "status": s, "conclusion": c
            })).collect::<Vec<_>>()}))
            .state
        };
        assert_eq!(runs(vec![]), CheckState::None);
        assert_eq!(
            runs(vec![
                ("completed", Some("success")),
                ("completed", Some("skipped"))
            ]),
            CheckState::Success
        );
        assert_eq!(
            runs(vec![("completed", Some("success")), ("in_progress", None)]),
            CheckState::Pending
        );
        assert_eq!(
            runs(vec![("in_progress", None), ("completed", Some("failure"))]),
            CheckState::Failure
        );
        assert_eq!(
            runs(vec![("completed", Some("cancelled"))]),
            CheckState::Failure
        );
    }

    #[test]
    fn api_errors_are_readable_and_a_401_means_sign_in_again() {
        assert_eq!(api_error(401, "{}"), GithubError::Unauthorized);
        let e = api_error(
            422,
            r#"{"message":"Validation Failed","errors":[{"message":"A pull request already exists for a:b."}]}"#,
        );
        assert_eq!(
            e.to_string(),
            "Validation Failed: A pull request already exists for a:b."
        );
        assert!(api_error(500, "not json").to_string().contains("HTTP 500"));
    }
}
