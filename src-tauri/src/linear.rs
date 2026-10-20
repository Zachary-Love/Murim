use serde::Serialize;
use serde_json::{json, Value};
use crate::reviews::{pr_url, Assignee, LinkedIssue};

/// A Linear "blocks" relation pointing at this issue. Done = blocker completed, canceled or duplicate.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    pub id: String,
    pub identifier: String,
    pub done: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub url: String,
    pub branch_name: String,
    pub state_name: String,
    pub state_type: String,
    pub priority: u8,
    pub labels: Vec<String>,
    pub team_key: String,
    pub updated_at: String,
    pub blockers: Vec<Blocker>,
}

// Linear's schema: WorkflowState.type is one of triage, backlog, unstarted, started,
// completed, canceled, duplicate. Duplicates are the native "duplicate" state type.
const QUERY: &str = r#"query($after: String) { viewer { assignedIssues(first: 100, after: $after, filter: { cycle: { isActive: { eq: true } }, state: { type: { nin: ["completed", "canceled", "duplicate"] } } }) { pageInfo { hasNextPage endCursor } nodes { id identifier title description url branchName priority updatedAt state { name type } labels(first: 10) { nodes { name } } team { key } inverseRelations(first: 10) { nodes { type issue { id identifier state { type } } } } } } } }"#;

fn post(api_key: &str, body: Value) -> Result<Value, String> {
    // ureq returns Err on non-2xx; Linear sends GraphQL errors (e.g. RATELIMITED) with 400, so read them.
    let res = match ureq::post("https://api.linear.app/graphql").set("Authorization", api_key).timeout(std::time::Duration::from_secs(30)).send_json(body) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(e.to_string()),
    };
    res.into_json().map_err(|e| e.to_string())
}

pub fn fetch_cycle_issues(api_key: &str) -> Result<Vec<Issue>, String> {
    let (mut all, mut after) = (vec![], None);
    loop {
        let body = json!({ "query": QUERY, "variables": { "after": after } });
        let v = post(api_key, body)?;
        let (mut page, next) = parse(&v)?;
        all.append(&mut page);
        match next {
            Some(c) => after = Some(c),
            None => return Ok(all),
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub description: String,
    pub state_name: String,
    pub priority_label: String,
    pub labels: Vec<String>,
    pub assignee: String,
    pub comments: Vec<Comment>,
}

const DETAIL: &str = r#"query($id: String!) { issue(id: $id) { description priorityLabel state { name } assignee { name } labels { nodes { name } } comments(first: 50) { nodes { body createdAt user { name } } } } }"#;

#[tauri::command]
pub async fn get_issue_detail(issue_id: String) -> Result<IssueDetail, String> {
    let key = api_key()?;
    // ureq is blocking: keep it off the async runtime.
    let v = tauri::async_runtime::spawn_blocking(move || post(&key, json!({ "query": DETAIL, "variables": { "id": issue_id } })))
        .await
        .map_err(|e| e.to_string())??;
    parse_detail(&v)
}

/// Last `n` comments, oldest first: Pavilion's judgement input. Blocking (call off the async runtime).
pub fn recent_comments(key: &str, issue_id: &str, n: usize) -> Result<Vec<Comment>, String> {
    let mut c = parse_detail(&post(key, json!({ "query": DETAIL, "variables": { "id": issue_id } }))?)?.comments;
    c.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(c.split_off(c.len().saturating_sub(n)))
}

fn parse_detail(json: &Value) -> Result<IssueDetail, String> {
    if let Some(errs) = json.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty()) {
        let msg: Vec<&str> = errs.iter().filter_map(|e| e["message"].as_str()).collect();
        return Err(format!("Linear: {}", msg.join("; ")));
    }
    let n = &json["data"]["issue"];
    if n.is_null() {
        return Err("Linear: unexpected response".into());
    }
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    Ok(IssueDetail {
        description: s(&n["description"]),
        state_name: s(&n["state"]["name"]),
        priority_label: s(&n["priorityLabel"]),
        labels: n["labels"]["nodes"].as_array().into_iter().flatten().map(|l| s(&l["name"])).collect(),
        assignee: s(&n["assignee"]["name"]),
        comments: n["comments"]["nodes"].as_array().into_iter().flatten()
            .map(|c| Comment { author: s(&c["user"]["name"]), body: s(&c["body"]), created_at: s(&c["createdAt"]) })
            .collect(),
    })
}

fn parse(json: &Value) -> Result<(Vec<Issue>, Option<String>), String> {
    if let Some(errs) = json.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty()) {
        let msg: Vec<&str> = errs.iter().filter_map(|e| e["message"].as_str()).collect();
        let code = errs[0]["extensions"]["code"].as_str().unwrap_or("");
        return Err(format!("Linear: {} {}", code, msg.join("; ")).trim().to_string());
    }
    let conn = &json["data"]["viewer"]["assignedIssues"];
    let nodes = conn["nodes"].as_array().ok_or("Linear: unexpected response")?;
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let issues = nodes
        .iter()
        .filter(|n| !matches!(n["state"]["type"].as_str(), Some("completed" | "canceled" | "duplicate")))
        .map(|n| Issue {
            id: s(&n["id"]),
            identifier: s(&n["identifier"]),
            title: s(&n["title"]),
            description: s(&n["description"]), // null -> ""
            url: s(&n["url"]),
            branch_name: s(&n["branchName"]),
            state_name: s(&n["state"]["name"]),
            state_type: s(&n["state"]["type"]),
            priority: n["priority"].as_u64().unwrap_or(0) as u8,
            labels: n["labels"]["nodes"].as_array().into_iter().flatten().map(|l| s(&l["name"])).collect(),
            team_key: s(&n["team"]["key"]),
            updated_at: s(&n["updatedAt"]),
            // inverseRelations of X with type "blocks": node.issue blocks X
            blockers: n["inverseRelations"]["nodes"].as_array().into_iter().flatten().filter(|r| r["type"] == "blocks")
                .map(|r| Blocker { id: s(&r["issue"]["id"]), identifier: s(&r["issue"]["identifier"]), done: matches!(r["issue"]["state"]["type"].as_str(), Some("completed" | "canceled" | "duplicate")) })
                .collect(),
        })
        .collect();
    let next = conn["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false).then(|| s(&conn["pageInfo"]["endCursor"]));
    Ok((issues, next))
}

const MY_TEAMS: &str = r#"query { viewer { teams { nodes { id } } } }"#;
const TEAM_PRS: &str = r#"query($teams: [ID!]) { issues(first: 100, filter: { team: { id: { in: $teams } }, state: { type: { eq: "started" } } }) { nodes { identifier url assignee { id name avatarUrl } attachments { nodes { url } } } } }"#;
const BY_IDENT: &str = r#"query($id: String!) { issue(id: $id) { identifier url assignee { id name avatarUrl } } }"#;

pub fn api_key() -> Result<String, String> {
    crate::settings::entry()?.get_password().map_err(|_| "Add your Linear API key in Settings".to_string())
}

fn gql_err(json: &Value) -> Option<String> {
    let errs = json.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty())?;
    Some(format!("Linear: {}", errs.iter().filter_map(|e| e["message"].as_str()).collect::<Vec<_>>().join("; ")))
}

fn assignee(v: &Value) -> Option<Assignee> {
    Some(Assignee { id: v["id"].as_str()?.into(), name: v["name"].as_str().unwrap_or_default().into(), avatar_url: v["avatarUrl"].as_str().map(String::from) })
}

fn linked(n: &Value) -> LinkedIssue {
    LinkedIssue { identifier: n["identifier"].as_str().unwrap_or_default().into(), url: n["url"].as_str().unwrap_or_default().into() }
}

fn parse_team_links(json: &Value) -> Result<Vec<(String, LinkedIssue, Option<Assignee>)>, String> {
    if let Some(e) = gql_err(json) {
        return Err(e);
    }
    let mut out = vec![];
    for n in json["data"]["issues"]["nodes"].as_array().ok_or("Linear: unexpected response")? {
        for a in n["attachments"]["nodes"].as_array().into_iter().flatten() {
            let url = a["url"].as_str().unwrap_or_default();
            if pr_url(url).is_some() {
                out.push((url.to_string(), linked(n), assignee(&n["assignee"])));
            }
        }
    }
    Ok(out)
}

fn parse_ident(json: &Value) -> Result<Option<(LinkedIssue, Option<Assignee>)>, String> {
    match gql_err(json) {
        Some(e) if e.contains("not found") => return Ok(None),
        Some(e) => return Err(e),
        None => {}
    }
    let n = &json["data"]["issue"];
    Ok((!n.is_null()).then(|| (linked(n), assignee(&n["assignee"]))))
}

/// PRs attached to started issues in the viewer's teams. ponytail: first 100 issues; page if teams outgrow it.
pub fn team_pr_links(key: &str) -> Result<Vec<(String, LinkedIssue, Option<Assignee>)>, String> {
    let t = post(key, json!({ "query": MY_TEAMS }))?;
    if let Some(e) = gql_err(&t) {
        return Err(e);
    }
    let teams: Vec<&str> = t["data"]["viewer"]["teams"]["nodes"].as_array().into_iter().flatten().filter_map(|n| n["id"].as_str()).collect();
    parse_team_links(&post(key, json!({ "query": TEAM_PRS, "variables": { "teams": teams } }))?)
}

pub fn issue_by_ident(key: &str, ident: &str) -> Result<Option<(LinkedIssue, Option<Assignee>)>, String> {
    parse_ident(&post(key, json!({ "query": BY_IDENT, "variables": { "id": ident } }))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, ty: &str) -> Value {
        json!({ "id": id, "identifier": format!("ENG-{id}"), "title": "t", "description": null, "url": "u", "branchName": "b", "priority": 2,
            "state": { "name": "S", "type": ty }, "labels": { "nodes": [{ "name": "bug" }, { "name": "ui" }] }, "team": { "key": "ENG" } })
    }

    #[test]
    fn parse_filters_and_flattens() {
        let j = json!({ "data": { "viewer": { "assignedIssues": {
            "pageInfo": { "hasNextPage": true, "endCursor": "c1" },
            "nodes": [node("1", "started"), node("2", "completed"), node("3", "canceled"), node("4", "duplicate")] } } } });
        let (issues, next) = parse(&j).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].identifier, "ENG-1");
        assert_eq!(issues[0].labels, ["bug", "ui"]);
        assert_eq!(issues[0].team_key, "ENG");
        assert_eq!(issues[0].description, "");
        assert_eq!(next.as_deref(), Some("c1"));
        let e = json!({ "errors": [{ "message": "slow down", "extensions": { "code": "RATELIMITED" } }] });
        assert!(parse(&e).unwrap_err().contains("RATELIMITED"));
    }

    #[test]
    fn detail_parses() {
        let j = json!({ "data": { "issue": { "description": null, "priorityLabel": "High", "state": { "name": "In Progress" },
            "assignee": null, "labels": { "nodes": [{ "name": "bug" }] },
            "comments": { "nodes": [{ "body": "hi", "createdAt": "2024-01-01T00:00:00Z", "user": { "name": "Ann" } }] } } } });
        let d = parse_detail(&j).unwrap();
        assert_eq!((d.description.as_str(), d.priority_label.as_str(), d.assignee.as_str()), ("", "High", ""));
        assert_eq!(d.labels, ["bug"]);
        assert_eq!(d.comments[0].author, "Ann");
        assert_eq!(serde_json::to_value(&d.comments[0]).unwrap()["createdAt"], "2024-01-01T00:00:00Z");
        assert!(parse_detail(&json!({ "data": { "issue": null } })).is_err());
    }

    #[test]
    fn team_links_keep_only_pr_attachments() {
        let j = json!({"data": {"issues": {"nodes": [
            {"identifier": "ENG-1", "url": "l1", "assignee": {"id": "u1", "name": "Amy", "avatarUrl": "https://a"},
             "attachments": {"nodes": [{"url": "https://github.com/o/r/pull/4"}, {"url": "https://figma.com/x"}]}},
            {"identifier": "ENG-2", "url": "l2", "assignee": null, "attachments": {"nodes": []}}
        ]}}});
        let v = parse_team_links(&j).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "https://github.com/o/r/pull/4");
        assert_eq!(v[0].1.identifier, "ENG-1");
        assert_eq!(v[0].2.as_ref().unwrap().avatar_url.as_deref(), Some("https://a"));
        assert!(parse_team_links(&json!({"errors": [{"message": "bad"}]})).is_err());
    }

    #[test]
    fn issue_by_ident_parse() {
        let j = json!({"data": {"issue": {"identifier": "ENG-9", "url": "l", "assignee": {"id": "u", "name": "Bo", "avatarUrl": null}}}});
        let (i, a) = parse_ident(&j).unwrap().unwrap();
        assert_eq!((i.identifier.as_str(), a.unwrap().name.as_str()), ("ENG-9", "Bo"));
        assert!(parse_ident(&json!({"data": {"issue": null}})).unwrap().is_none());
        // Linear answers an unknown identifier with an "Entity not found" error: treat as no issue
        assert!(parse_ident(&json!({"errors": [{"message": "Entity not found: Issue"}]})).unwrap().is_none());
    }

    #[test]
    fn parse_blockers_and_updated_at() {
        let mut n = node("1", "started");
        n["updatedAt"] = json!("2026-10-19T10:00:00Z");
        n["inverseRelations"] = json!({"nodes": [
            {"type": "blocks", "issue": {"id": "b1", "identifier": "ENG-9", "state": {"type": "started"}}},
            {"type": "blocks", "issue": {"id": "b2", "identifier": "ENG-8", "state": {"type": "completed"}}},
            {"type": "related", "issue": {"id": "b3", "identifier": "ENG-7", "state": {"type": "started"}}}]});
        let j = json!({"data": {"viewer": {"assignedIssues": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": [n]}}}});
        let (v, _) = parse(&j).unwrap();
        assert_eq!(v[0].updated_at, "2026-10-19T10:00:00Z");
        assert_eq!(v[0].blockers, vec![Blocker { id: "b1".into(), identifier: "ENG-9".into(), done: false }, Blocker { id: "b2".into(), identifier: "ENG-8".into(), done: true }]);
        // no relations field: no blockers
        let j = json!({"data": {"viewer": {"assignedIssues": {"pageInfo": {}, "nodes": [node("2", "started")]}}}});
        assert!(parse(&j).unwrap().0[0].blockers.is_empty());
    }
}
