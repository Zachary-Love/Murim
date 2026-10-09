// THROWAWAY spike. usage: spike <worktree> <logprefix> <steps...>
// steps: "spawn:<extra claude args>" | "wait:<secs>" | "type:<text>" | "enter" | "key:<hex>" | "waitev:<Event>:<secs>"
use portable_pty::*;
use std::{io::*, sync::*, time::*, fs};
fn ts() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64() }
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (wt, pre) = (&a[1], &a[2]);
    let token = format!("{:016x}", rand::random::<u64>());
    let srv = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = srv.server_addr().to_ip().unwrap().port();
    let events: Arc<Mutex<Vec<(f64, String)>>> = Default::default();
    let hl = Arc::new(Mutex::new(fs::OpenOptions::new().create(true).append(true).open(format!("{pre}.hooks.log")).unwrap()));
    { let (srv, ev, tok, hl) = (srv.clone(), events.clone(), token.clone(), hl.clone());
      std::thread::spawn(move || for mut r in srv.incoming_requests() {
        let hv = |r: &tiny_http::Request, n: &'static str| r.headers().iter().find(|h| h.field.equiv(n)).map(|h| h.value.to_string()).unwrap_or_default();
        let (t, task) = (hv(&r, "X-Murim-Token"), hv(&r, "X-Murim-Task"));
        let mut b = String::new(); r.as_reader().read_to_string(&mut b).unwrap();
        let ok = t == tok;
        let name = serde_json::from_str::<serde_json::Value>(&b).ok().and_then(|v| v["hook_event_name"].as_str().map(String::from)).unwrap_or("?".into());
        writeln!(hl.lock().unwrap(), "{:.3} tokok={ok} task={task} {b}", ts()).unwrap();
        if ok { ev.lock().unwrap().push((ts(), name)); }
        r.respond(tiny_http::Response::empty(200)).ok();
      }); }
    // settings
    let hook = r#"curl -s -X POST http://127.0.0.1:$PORT/hook -H "X-Murim-Token: $TOKEN" -H "X-Murim-Task: $TASK" --data-binary @-"#;
    let mut hooks = serde_json::Map::new();
    for e in ["UserPromptSubmit","PreToolUse","Notification","Stop","SessionEnd"] {
        hooks.insert(e.into(), serde_json::json!([{"matcher": if e=="PreToolUse"||e=="Notification" {"*"} else {""}, "hooks":[{"type":"command","command":hook}]}]));
    }
    fs::create_dir_all(format!("{wt}/.claude")).unwrap();
    fs::write(format!("{wt}/.claude/settings.local.json"), serde_json::json!({"hooks":hooks}).to_string()).unwrap();

    let pty = native_pty_system().openpty(PtySize{rows:40,cols:120,pixel_width:0,pixel_height:0}).unwrap();
    let mut writer = pty.master.take_writer().unwrap();
    let mut reader = pty.master.try_clone_reader().unwrap();
    let mut log = fs::File::create(format!("{pre}.pty.log")).unwrap();
    std::thread::spawn(move || { let mut buf=[0u8;8192]; while let Ok(n)=reader.read(&mut buf){ if n==0{break} log.write_all(format!("\n<<{:.3}>>",ts()).as_bytes()).ok(); log.write_all(&buf[..n]).ok(); } });
    let mut child: Option<Box<dyn Child+Send+Sync>> = None;
    for s in &a[3..] {
        let (k, v) = s.split_once(':').unwrap_or((s, ""));
        eprintln!("{:.3} step {s}", ts());
        match k {
            "spawn" => { let mut c = CommandBuilder::new("/Users/zacharylove/.local/bin/claude");
                c.args(["--model","haiku"]); for x in v.split_whitespace(){c.arg(x)}
                for (k,_) in std::env::vars() { if k.starts_with("CLAUDE") || k=="AI_AGENT" { c.env_remove(k); } } c.cwd(wt); c.env("PORT",port.to_string()); c.env("TOKEN",&token); c.env("TASK","task-1"); c.env("TERM","xterm-256color");
                child = Some(pty.slave.spawn_command(c).unwrap()); }
            "spawnq" => { // spawn with a single quoted-free arg list split on '|'
                let mut c = CommandBuilder::new("/Users/zacharylove/.local/bin/claude");
                c.args(["--model","haiku"]); for x in v.split('|'){c.arg(x)}
                for (k,_) in std::env::vars() { if k.starts_with("CLAUDE") || k=="AI_AGENT" { c.env_remove(k); } } c.cwd(wt); c.env("PORT",port.to_string()); c.env("TOKEN",&token); c.env("TASK","task-1"); c.env("TERM","xterm-256color");
                child = Some(pty.slave.spawn_command(c).unwrap()); }
            "wait" => std::thread::sleep(Duration::from_secs_f64(v.parse().unwrap())),
            "type" => { writer.write_all(v.as_bytes()).unwrap(); writer.flush().unwrap(); }
            "enter" => { writer.write_all(b"\r").unwrap(); writer.flush().unwrap(); }
            "key" => { let b: Vec<u8> = (0..v.len()).step_by(2).map(|i| u8::from_str_radix(&v[i..i+2],16).unwrap()).collect(); writer.write_all(&b).unwrap(); writer.flush().unwrap(); }
            "waitev" => { let (e, secs) = v.split_once(':').unwrap(); let t0 = Instant::now();
                while t0.elapsed().as_secs_f64() < secs.parse().unwrap() { if events.lock().unwrap().iter().any(|x| x.1==e) {break} std::thread::sleep(Duration::from_millis(200)); }
                eprintln!("  waitev {e}: {}", events.lock().unwrap().iter().any(|x| x.1==e)); }
            _ => panic!("bad step"),
        }
    }
    eprintln!("events: {:?}", events.lock().unwrap().iter().map(|x| &x.1).collect::<Vec<_>>());
    if let Some(mut c) = child { c.kill().ok(); }
}
