use serde::Serialize;
use serde_json::{json, Value};

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
}

// Linear's schema: WorkflowState.type is one of triage, backlog, unstarted, started,
// completed, canceled, duplicate. Duplicates are the native "duplicate" state type.
const QUERY: &str = r#"query($after: String) { viewer { assignedIssues(first: 100, after: $after, filter: { cycle: { isActive: { eq: true } }, state: { type: { nin: ["completed", "canceled", "duplicate"] } } }) { pageInfo { hasNextPage endCursor } nodes { id identifier title description url branchName priority state { name type } labels { nodes { name } } team { key } } } } }"#;

fn post(api_key: &str, body: Value) -> Result<Value, String> {
    // ureq returns Err on non-2xx; Linear sends GraphQL errors (e.g. RATELIMITED) with 400, so read them.
    let res = match ureq::post("https://api.linear.app/graphql").set("Authorization", api_key).send_json(body) {
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
    let key = crate::settings::entry()?.get_password().map_err(|_| "Add your Linear API key in Settings".to_string())?;
    // ureq is blocking: keep it off the async runtime.
    let v = tauri::async_runtime::spawn_blocking(move || post(&key, json!({ "query": DETAIL, "variables": { "id": issue_id } })))
        .await
        .map_err(|e| e.to_string())??;
    parse_detail(&v)
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
        })
        .collect();
    let next = conn["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false).then(|| s(&conn["pageInfo"]["endCursor"]));
    Ok((issues, next))
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
}
