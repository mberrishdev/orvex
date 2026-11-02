//! Background commands are owned by the guest, so closing the MCP client does
//! not stop a dev server. Session records and output stay on its VM disk.
use crate::workspace_controller::WorkspaceController;
use serde_json::{Value, json};

pub(crate) fn execute(
    controller: &WorkspaceController,
    id: &str,
    network: bool,
    command: &[String],
    timeout: u64,
) -> Result<Value, String> {
    use std::{
        io::Read,
        process::Stdio,
        time::{Duration, Instant},
    };
    if !(1..=120).contains(&timeout) {
        return Err(
            "timeoutSeconds must be between 1 and 120. Use a command session for longer work."
                .into(),
        );
    }
    if command.is_empty() {
        return Err("command must not be empty.".into());
    }
    // Drain both pipes concurrently to avoid deadlock; retain bounded output.
    fn capture(mut stream: impl Read) -> (String, bool) {
        let mut retained = Vec::new();
        let mut buffer = [0; 8192];
        let mut truncated = false;
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let keep = count.min(65536usize.saturating_sub(retained.len()));
            retained.extend_from_slice(&buffer[..keep]);
            truncated |= keep < count;
        }
        (String::from_utf8_lossy(&retained).into_owned(), truncated)
    }
    // Guest timeout kills the command's group even if the host connection drops.
    // Wrapping argv preserves argument boundaries and never expands shell text.
    let mut bounded = vec![
        "timeout".into(),
        "--signal=TERM".into(),
        "--kill-after=2s".into(),
        format!("{timeout}s"),
    ];
    bounded.extend_from_slice(command);
    let mut invocation = controller.command(id, network, &bounded)?;
    let mut child = invocation
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || capture(stdout));
    let err = std::thread::spawn(move || capture(stderr));
    let deadline = Instant::now() + Duration::from_secs(timeout + 15);
    let (status, host_timeout) = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break (status, false);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            break (child.wait().map_err(|error| error.to_string())?, true);
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    // A descendant retaining a pipe must not block the MCP transport forever.
    let join_deadline = Instant::now() + Duration::from_secs(2);
    while (!out.is_finished() || !err.is_finished()) && Instant::now() < join_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let (stdout, out_truncated) = if out.is_finished() {
        out.join().unwrap_or_default()
    } else {
        (
            "Output stream remains open; use a background session.".into(),
            true,
        )
    };
    let (stderr, err_truncated) = if err.is_finished() {
        err.join().unwrap_or_default()
    } else {
        ("Error stream remains open.".into(), true)
    };
    Ok(
        json!({"workspaceId":id,"executionLocation":"guest","networkGranted":network,
        "exitCode":status.code(),"timedOut":host_timeout || status.code()==Some(124),
        "stdout":stdout,"stderr":stderr,"truncated":out_truncated || err_truncated}),
    )
}

fn session_id(id: &str) -> Result<(), String> {
    if !id.starts_with("s-")
        || id.len() > 80
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Invalid command session ID.".into());
    }
    Ok(())
}

pub(crate) fn start(
    controller: &WorkspaceController,
    workspace: &str,
    network: bool,
    argv: &[String],
) -> Result<Value, String> {
    if argv.is_empty() {
        return Err("command must contain at least one argument.".into());
    }
    let id = format!(
        "s-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    let worker = r#"set -eu
umask 077
dir="$HOME/.orvex/commands/$1"
shift
mkdir -p "$dir"
printf '%s\n' "$$" > "$dir/pid"
cat /proc/sys/kernel/random/boot_id > "$dir/boot"
awk '{print $22}' /proc/$$/stat > "$dir/start"
printf 'running\n' > "$dir/state"
# Limit command output growth to 32 MiB per stream.
ulimit -f 32768
set +e
"$@"
code=$?
set -e
printf '%s\n' "$code" > "$dir/exit.tmp"
mv "$dir/exit.tmp" "$dir/exit"
printf 'exited\n' > "$dir/state"
"#;
    let launch = r#"set -eu
umask 077
dir="$HOME/.orvex/commands/$1"
mkdir -p "$dir"
id="$1"
worker="$2"
printf '%s\n' "$3" > "$dir/network"
shift 3
nohup setsid bash -c "$worker" orvex-session "$id" "$@" > "$dir/stdout" 2> "$dir/stderr" < /dev/null &
for attempt in $(seq 1 40); do
  if [ -f "$dir/state" ]; then printf '%s\n' "$id"; exit 0; fi
  sleep 0.05
done
printf 'Background command failed to start.\n' >&2
cat "$dir/stderr" >&2
exit 1
"#;
    let mut command = vec![
        "bash".into(),
        "-c".into(),
        launch.into(),
        "orvex-launch".into(),
        id.clone(),
        worker.into(),
        network.to_string(),
    ];
    command.extend_from_slice(argv);
    let result = execute(controller, workspace, network, &command, 15)?;
    if result["exitCode"] != 0 {
        return Err(format!("Could not launch command session: {result}"));
    }
    Ok(json!({"workspaceId":workspace,"sessionId":id,"networkGranted":network,"state":"starting"}))
}

pub(crate) fn inspect(
    controller: &WorkspaceController,
    workspace: &str,
    id: &str,
    stop: bool,
) -> Result<Value, String> {
    session_id(id)?;
    let script = r#"dir="$HOME/.orvex/commands/$1"
test -d "$dir" || { printf 'Session not found\n' >&2; exit 1; }
state=starting
test ! -f "$dir/state" || state="$(cat "$dir/state")"
pid="$(cat "$dir/pid" 2>/dev/null || true)"
live=false
if [ -n "$pid" ] && [ "$(cat "$dir/boot" 2>/dev/null)" = "$(cat /proc/sys/kernel/random/boot_id)" ] && [ "$(cat "$dir/start" 2>/dev/null)" = "$(awk '{print $22}' "/proc/$pid/stat" 2>/dev/null)" ]; then
  live=true
fi
if [ "$2" = stop ] && [ "$live" = true ]; then
  kill -TERM -- "-$pid" 2>/dev/null || true
  sleep 1
  if [ "$(cat "$dir/start")" = "$(awk '{print $22}' "/proc/$pid/stat" 2>/dev/null)" ]; then kill -KILL -- "-$pid" 2>/dev/null || true; fi
  printf 'cancelled\n' > "$dir/state"
  state=cancelled
elif [ "$live" = false ] && [ "$state" = running ]; then state=interrupted; fi
printf 'state=%s\nexitCode=%s\nnetwork=%s\n' "$state" "$(cat "$dir/exit" 2>/dev/null || true)" "$(cat "$dir/network" 2>/dev/null || true)"
printf '\n--- stdout (last 32 KiB) ---\n'
tail -c 32768 "$dir/stdout" 2>/dev/null || true
printf '\n--- stderr (last 32 KiB) ---\n'
tail -c 32768 "$dir/stderr" 2>/dev/null || true
"#;
    // Guest process management is local-only; no network grant is inherited.
    let result = execute(
        controller,
        workspace,
        false,
        &[
            "bash".into(),
            "-c".into(),
            script.into(),
            "orvex-session-status".into(),
            id.into(),
            if stop { "stop" } else { "status" }.into(),
        ],
        10,
    )?;
    if result["exitCode"] != 0 {
        return Err(format!("Could not inspect session: {result}"));
    }
    let output = result["stdout"].as_str().unwrap_or_default();
    let mut lines = output.lines();
    let state = lines
        .next()
        .unwrap_or("state=unknown")
        .strip_prefix("state=")
        .unwrap_or("unknown");
    let exit_code = lines
        .next()
        .and_then(|s| s.strip_prefix("exitCode="))
        .and_then(|s| s.parse::<i32>().ok());
    let network_granted = lines
        .next()
        .and_then(|s| s.strip_prefix("network="))
        .map(|s| s == "true");
    let logs = lines.collect::<Vec<_>>().join("\n");
    Ok(
        json!({"workspaceId":workspace,"sessionId":id,"state":state,"exitCode":exit_code,"logs":logs,"logsTruncated":result["truncated"],"networkGranted":network_granted}),
    )
}
