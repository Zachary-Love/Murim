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

pub fn fetch_cycle_issues(api_key: &str) -> Result<Vec<Issue>, String> {
    let (mut all, mut after) = (vec![], None);
    loop {
        let body = json!({ "query": QUERY, "variables": { "after": after } });
        // ureq returns Err on non-2xx; Linear sends GraphQL errors (e.g. RATELIMITED) with 400, so read them.
        let res = match ureq::post("https://api.linear.app/graphql").set("Authorization", api_key).send_json(body) {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => return Err(e.to_string()),
        };
        let v: Value = res.into_json().map_err(|e| e.to_string())?;
        let (mut page, next) = parse(&v)?;
        all.append(&mut page);
        match next {
            Some(c) => after = Some(c),
            None => return Ok(all),
        }
    }
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
}
