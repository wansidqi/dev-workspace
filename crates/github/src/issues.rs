//! GitHub issues: list, create, edit, close and reopen, comments, and the
//! labels, assignees and milestones an issue can carry. GitHub is the source of
//! truth; nothing here is cached, the app re-reads on demand.

use ureq::serde_json::{self, Value};

use crate::{agent, finish, repo_path, GithubError, RepoRef};

/// Issues per page; a full page means there may be more.
pub const PAGE_SIZE: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
}

/// Which issues a listing asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Open,
    Closed,
    All,
}

impl StateFilter {
    fn query(self) -> &'static str {
        match self {
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
            StateFilter::All => "all",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    /// Hex without the `#`, as GitHub sends it.
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Milestone {
    pub number: u64,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// GitHub database id; the sub-issues API takes this, not the number.
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub author: String,
    pub labels: Vec<Label>,
    pub assignees: Vec<String>,
    pub milestone: Option<Milestone>,
    pub comments: u64,
    /// The issue this one is a sub-issue of, in the same repo.
    pub parent_number: Option<u64>,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn login(v: &Value) -> String {
    v.get("user")
        .and_then(|u| text(u, "login"))
        .unwrap_or_default()
}

fn parse_label(v: &Value) -> Option<Label> {
    Some(Label {
        name: text(v, "name")?,
        color: text(v, "color").unwrap_or_else(|| "888888".into()),
    })
}

fn parse_milestone(v: &Value) -> Option<Milestone> {
    Some(Milestone {
        number: v.get("number")?.as_u64()?,
        title: text(v, "title")?,
    })
}

fn parse_issue(v: &Value) -> Option<Issue> {
    let state = match v.get("state").and_then(Value::as_str)? {
        "closed" => IssueState::Closed,
        _ => IssueState::Open,
    };
    Some(Issue {
        id: v.get("id")?.as_u64()?,
        number: v.get("number")?.as_u64()?,
        title: text(v, "title")?,
        // `body` is null for an issue created without one.
        body: text(v, "body").unwrap_or_default(),
        state,
        author: login(v),
        labels: v
            .get("labels")
            .and_then(Value::as_array)
            .map(|l| l.iter().filter_map(parse_label).collect())
            .unwrap_or_default(),
        assignees: v
            .get("assignees")
            .and_then(Value::as_array)
            .map(|l| l.iter().filter_map(|a| text(a, "login")).collect())
            .unwrap_or_default(),
        milestone: v.get("milestone").and_then(parse_milestone),
        comments: v.get("comments").and_then(Value::as_u64).unwrap_or(0),
        parent_number: text(v, "parent_issue_url").and_then(|u| parse_trailing_number(&u)),
        url: text(v, "html_url")?,
        created_at: text(v, "created_at").unwrap_or_default(),
        updated_at: text(v, "updated_at").unwrap_or_default(),
    })
}

fn parse_comment(v: &Value) -> Option<Comment> {
    Some(Comment {
        id: v.get("id")?.as_u64()?,
        author: login(v),
        body: text(v, "body").unwrap_or_default(),
        created_at: text(v, "created_at").unwrap_or_default(),
    })
}

/// The issues in a listing. GitHub's issues endpoint also returns pull
/// requests (an item with a `pull_request` key); those are not issues here.
fn parse_issue_list(v: &Value) -> Vec<Issue> {
    v.as_array()
        .map(|list| {
            list.iter()
                .filter(|i| i.get("pull_request").is_none())
                .filter_map(parse_issue)
                .collect()
        })
        .unwrap_or_default()
}

fn authed(request: ureq::Request, token: &str) -> ureq::Request {
    request
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .set("Authorization", &format!("Bearer {token}"))
}

fn send(token: &str, method: &str, url: &str, body: Value) -> Result<Value, GithubError> {
    finish(authed(agent().request(method, url), token).send_json(body))
}

fn get(token: &str, url: &str) -> Result<Value, GithubError> {
    finish(authed(agent().get(url), token).call())
}

/// One page (1-based) of issues, most recently updated first.
pub fn list_issues(
    token: &str,
    repo: &RepoRef,
    filter: StateFilter,
    page: u32,
) -> Result<Vec<Issue>, GithubError> {
    let url = format!(
        "{}/issues?state={}&sort=updated&direction=desc&per_page={PAGE_SIZE}&page={}",
        repo_path(repo)?,
        filter.query(),
        page.max(1)
    );
    Ok(parse_issue_list(&get(token, &url)?))
}

/// What to open an issue with.
#[derive(Debug, Clone, Default)]
pub struct NewIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<u64>,
}

pub fn create_issue(token: &str, repo: &RepoRef, issue: &NewIssue) -> Result<Issue, GithubError> {
    if issue.title.trim().is_empty() {
        return Err(GithubError::Invalid("write a title for the issue".into()));
    }
    let mut body = serde_json::json!({
        "title": issue.title.trim(),
        "body": issue.body,
        "labels": issue.labels,
        "assignees": issue.assignees,
    });
    if let Some(m) = issue.milestone {
        body["milestone"] = m.into();
    }
    let v = send(token, "POST", &format!("{}/issues", repo_path(repo)?), body)?;
    parse_issue(&v).ok_or_else(|| GithubError::Api("GitHub's answer wasn't an issue".into()))
}

/// Changes to an issue; a `None` field is left as it is.
#[derive(Debug, Clone, Default)]
pub struct IssueUpdate {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<IssueState>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    /// `Some(None)` clears the milestone.
    pub milestone: Option<Option<u64>>,
}

fn update_body(update: &IssueUpdate) -> Result<Value, GithubError> {
    let mut body = serde_json::Map::new();
    if let Some(title) = &update.title {
        if title.trim().is_empty() {
            return Err(GithubError::Invalid("an issue needs a title".into()));
        }
        body.insert("title".into(), title.trim().into());
    }
    if let Some(text) = &update.body {
        body.insert("body".into(), text.as_str().into());
    }
    match update.state {
        Some(IssueState::Closed) => {
            body.insert("state".into(), "closed".into());
            body.insert("state_reason".into(), "completed".into());
        }
        Some(IssueState::Open) => {
            body.insert("state".into(), "open".into());
            body.insert("state_reason".into(), "reopened".into());
        }
        None => {}
    }
    if let Some(labels) = &update.labels {
        body.insert("labels".into(), serde_json::json!(labels));
    }
    if let Some(assignees) = &update.assignees {
        body.insert("assignees".into(), serde_json::json!(assignees));
    }
    if let Some(milestone) = update.milestone {
        body.insert(
            "milestone".into(),
            milestone.map_or(Value::Null, Value::from),
        );
    }
    Ok(Value::Object(body))
}

/// Edits an issue: title, body, open or closed, labels, assignees, milestone.
pub fn update_issue(
    token: &str,
    repo: &RepoRef,
    number: u64,
    update: &IssueUpdate,
) -> Result<Issue, GithubError> {
    let url = format!("{}/issues/{number}", repo_path(repo)?);
    let v = send(token, "PATCH", &url, update_body(update)?)?;
    parse_issue(&v).ok_or_else(|| GithubError::Api("GitHub's answer wasn't an issue".into()))
}

/// The comments on an issue, oldest first (first page of 100).
pub fn list_comments(
    token: &str,
    repo: &RepoRef,
    number: u64,
) -> Result<Vec<Comment>, GithubError> {
    let url = format!("{}/issues/{number}/comments?per_page=100", repo_path(repo)?);
    let v = get(token, &url)?;
    Ok(v.as_array()
        .map(|list| list.iter().filter_map(parse_comment).collect())
        .unwrap_or_default())
}

pub fn add_comment(
    token: &str,
    repo: &RepoRef,
    number: u64,
    body: &str,
) -> Result<Comment, GithubError> {
    if body.trim().is_empty() {
        return Err(GithubError::Invalid("write a comment first".into()));
    }
    let url = format!("{}/issues/{number}/comments", repo_path(repo)?);
    let v = send(token, "POST", &url, serde_json::json!({ "body": body }))?;
    parse_comment(&v).ok_or_else(|| GithubError::Api("GitHub's answer wasn't a comment".into()))
}

/// Labels defined on the repo.
pub fn list_labels(token: &str, repo: &RepoRef) -> Result<Vec<Label>, GithubError> {
    let v = get(token, &format!("{}/labels?per_page=100", repo_path(repo)?))?;
    Ok(v.as_array()
        .map(|l| l.iter().filter_map(parse_label).collect())
        .unwrap_or_default())
}

/// Logins that can be assigned to issues in the repo.
pub fn list_assignees(token: &str, repo: &RepoRef) -> Result<Vec<String>, GithubError> {
    let v = get(
        token,
        &format!("{}/assignees?per_page=100", repo_path(repo)?),
    )?;
    Ok(v.as_array()
        .map(|l| l.iter().filter_map(|a| text(a, "login")).collect())
        .unwrap_or_default())
}

/// Open milestones of the repo.
pub fn list_milestones(token: &str, repo: &RepoRef) -> Result<Vec<Milestone>, GithubError> {
    let v = get(
        token,
        &format!("{}/milestones?state=open&per_page=100", repo_path(repo)?),
    )?;
    Ok(v.as_array()
        .map(|l| l.iter().filter_map(parse_milestone).collect())
        .unwrap_or_default())
}

/// The number at the end of an issue URL such as
/// `https://api.github.com/repos/o/r/issues/12`.
fn parse_trailing_number(url: &str) -> Option<u64> {
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// One issue by number.
pub fn get_issue(token: &str, repo: &RepoRef, number: u64) -> Result<Issue, GithubError> {
    let v = get(token, &format!("{}/issues/{number}", repo_path(repo)?))?;
    parse_issue(&v).ok_or_else(|| GithubError::Api("GitHub's answer wasn't an issue".into()))
}

/// Makes `sub_issue_id` (the child's database id, not its number) a
/// sub-issue of issue `parent`. GitHub allows 8 levels and 100 children.
pub fn add_sub_issue(
    token: &str,
    repo: &RepoRef,
    parent: u64,
    sub_issue_id: u64,
) -> Result<(), GithubError> {
    let url = format!("{}/issues/{parent}/sub_issues", repo_path(repo)?);
    send(
        token,
        "POST",
        &url,
        serde_json::json!({ "sub_issue_id": sub_issue_id }),
    )
    .map(|_| ())
}

/// Detaches a sub-issue from its parent.
pub fn remove_sub_issue(
    token: &str,
    repo: &RepoRef,
    parent: u64,
    sub_issue_id: u64,
) -> Result<(), GithubError> {
    let url = format!("{}/issues/{parent}/sub_issue", repo_path(repo)?);
    send(
        token,
        "DELETE",
        &url,
        serde_json::json!({ "sub_issue_id": sub_issue_id }),
    )
    .map(|_| ())
}

/// The issue number in a `#123` reference, if `text` is one.
pub fn parse_issue_ref(text: &str) -> Option<u64> {
    text.trim().strip_prefix('#')?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ureq::serde_json::json;

    #[test]
    fn issue_parses_with_nulls_and_drops_pull_requests() {
        let list = json!([
            {
                "id": 108, "number": 8, "title": "md editor is not wrap", "body": null,
                "parent_issue_url": "https://api.github.com/repos/o/r/issues/3",
                "state": "open", "user": {"login": "wansidqi"},
                "labels": [{"name": "bug", "color": "d73a4a"}],
                "assignees": [{"login": "wansidqi"}],
                "milestone": null, "comments": 2,
                "html_url": "https://github.com/o/r/issues/8",
                "created_at": "2026-11-24T00:00:00Z", "updated_at": "2026-11-25T00:00:00Z"
            },
            {
                "number": 9, "title": "a pull request", "state": "open",
                "pull_request": {"url": "x"}, "html_url": "https://github.com/o/r/pull/9"
            }
        ]);
        let issues = parse_issue_list(&list);
        assert_eq!(issues.len(), 1);
        let i = &issues[0];
        assert_eq!(i.number, 8);
        assert_eq!(i.body, "");
        assert_eq!(i.state, IssueState::Open);
        assert_eq!(i.labels[0].name, "bug");
        assert_eq!(i.assignees, vec!["wansidqi".to_string()]);
        assert_eq!(i.milestone, None);
        assert_eq!(i.comments, 2);
        assert_eq!(i.id, 108);
        assert_eq!(i.parent_number, Some(3));
    }

    #[test]
    fn an_issue_without_a_parent_has_none() {
        let v = json!({"id": 5, "number": 5, "title": "t", "state": "open", "html_url": "u"});
        assert_eq!(parse_issue(&v).unwrap().parent_number, None);
        assert_eq!(parse_trailing_number("https://x/issues/12/"), Some(12));
        assert_eq!(parse_trailing_number("https://x/issues/"), None);
    }

    #[test]
    fn closed_issue_and_milestone_parse() {
        let v = json!({
            "id": 101, "number": 1, "title": "t", "state": "closed",
            "milestone": {"number": 3, "title": "v1"},
            "html_url": "u"
        });
        let i = parse_issue(&v).unwrap();
        assert_eq!(i.state, IssueState::Closed);
        assert_eq!(
            i.milestone,
            Some(Milestone {
                number: 3,
                title: "v1".into()
            })
        );
    }

    #[test]
    fn update_sends_only_what_changed() {
        let body = update_body(&IssueUpdate {
            state: Some(IssueState::Closed),
            ..IssueUpdate::default()
        })
        .unwrap();
        assert_eq!(
            body,
            json!({"state": "closed", "state_reason": "completed"})
        );

        let body = update_body(&IssueUpdate {
            milestone: Some(None),
            labels: Some(vec!["bug".into()]),
            ..IssueUpdate::default()
        })
        .unwrap();
        assert_eq!(body["milestone"], Value::Null);
        assert_eq!(body["labels"], json!(["bug"]));
        assert!(body.get("title").is_none());
    }

    #[test]
    fn empty_titles_are_refused_before_any_request() {
        let repo = RepoRef {
            owner: "o".into(),
            name: "r".into(),
        };
        let err = create_issue("t", &repo, &NewIssue::default()).unwrap_err();
        assert!(matches!(err, GithubError::Invalid(_)));
        let err = update_body(&IssueUpdate {
            title: Some("  ".into()),
            ..IssueUpdate::default()
        })
        .unwrap_err();
        assert!(matches!(err, GithubError::Invalid(_)));
    }

    #[test]
    fn issue_references() {
        assert_eq!(parse_issue_ref("#42"), Some(42));
        assert_eq!(parse_issue_ref(" #7 "), Some(7));
        assert_eq!(parse_issue_ref("42"), None);
        assert_eq!(parse_issue_ref("#x"), None);
    }
}
