//! Static inspection of committed Git files. Never executes project code.
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Command};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Requirements {
    pub(crate) linux_compatibility: String,
    pub(crate) evidence: Vec<Value>,
    pub(crate) runtimes: Vec<Value>,
    pub(crate) suggested_commands: Vec<Value>,
    pub(crate) warnings: Vec<String>,
    needs: Vec<Value>,
    container_definitions: Vec<Value>,
    services: Vec<Value>,
    configuration_keys: Vec<Value>,
    commit: String,
    scope: &'static str,
    pub(crate) automatic_setup: bool,
}

fn relevant(path: &str) -> bool {
    if path.split('/').count() > 8
        || path
            .split('/')
            .any(|part| matches!(part, "node_modules" | "vendor" | "obj" | "bin"))
    {
        return false;
    }
    matches!(
        path.rsplit('/').next().unwrap_or(path),
        "package.json"
            | "Cargo.toml"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "Gemfile"
            | "Package.swift"
            | "project.pbxproj"
            | "compose.yaml"
            | "compose.yml"
            | "docker-compose.yaml"
            | "docker-compose.yml"
            | "Dockerfile"
            | "devcontainer.json"
            | ".nvmrc"
            | ".node-version"
            | ".python-version"
            | "mise.toml"
            | ".mise.toml"
            | "flake.nix"
            | "shell.nix"
            | "pnpm-lock.yaml"
            | "package-lock.json"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
    ) || path == ".orvex/workspace.json"
        || crate::project_configuration::relevant(path)
}

fn inspect(
    mut git: impl FnMut(&[&str]) -> Result<String, String>,
    reference: &str,
) -> Result<Requirements, String> {
    let commit = git(&["rev-parse", "--verify", &format!("{reference}^{{commit}}")])?
        .trim()
        .to_owned();
    let tree = git(&["ls-tree", "-r", "--name-only", "-z", &commit])?;
    let mut files = BTreeMap::new();
    let mut candidates = tree
        .split('\0')
        .filter(|s| !s.is_empty() && relevant(s))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| path.ends_with(".cs"));
    let truncated = candidates.len() > 256;
    candidates.truncate(256);
    for path in candidates {
        let object = format!("{commit}:{path}");
        let size = git(&["cat-file", "-s", &object])?
            .trim()
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        let content = if size <= 65536 {
            git(&["show", &object])?
        } else {
            String::new()
        };
        files.insert(path.to_owned(), content);
    }
    let mut report = analyze(commit, &files);
    if truncated {
        report.warnings.push("Inspection limited to 256 configuration/manifests/source files; additional dependencies may be missing. Configuration/manifests are prioritized over C# source.".into());
    }
    Ok(report)
}

pub(crate) fn local(project: &Path, reference: &str) -> Result<Requirements, String> {
    if !project.is_absolute() || !project.is_dir() {
        return Err("projectPath must be an existing absolute host directory.".into());
    }
    let git = crate::find_executable("git").ok_or("Git is required for project inspection.")?;
    inspect(
        |args| {
            let output = Command::new(&git)
                .arg("-C")
                .arg(project)
                .args(args)
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(crate::output_error(&output, "inspect project requirements"));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        },
        reference,
    )
}

pub(crate) fn guest(home: &Path, instance: &str, id: &str) -> Result<Requirements, String> {
    let (_, root, _) = crate::guest_user(home, instance)?;
    let root = root
        .join("orvex/workspaces")
        .join(id)
        .to_string_lossy()
        .into_owned();
    inspect(
        |args| {
            let mut argv = vec!["shell", instance, "git", "-C", root.as_str()];
            argv.extend_from_slice(args);
            let output = crate::run_limactl(home, &argv)?;
            if !output.status.success() {
                return Err(crate::output_error(
                    &output,
                    "inspect guest project requirements",
                ));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        },
        "HEAD",
    )
}

fn analyze(commit: String, files: &BTreeMap<String, String>) -> Requirements {
    let mut report=Requirements{linux_compatibility:"unknown".into(),evidence:Vec::new(),runtimes:Vec::new(),suggested_commands:Vec::new(),warnings:vec!["Static hints from committed files only; installed tools, runtime behavior, uncommitted files and external services are not verified. Suggested commands require review and are never executed by inspection.".into()],commit,scope:"committed Git tree; configuration/manifest/source paths at most eight components deep",automatic_setup:false,needs:Vec::new(),container_definitions:Vec::new(),services:Vec::new(),configuration_keys:Vec::new()};
    let mut apple = false;
    let mut portable = false;
    for (path, content) in files {
        let file = path.rsplit('/').next().unwrap();
        let mut evidence = |kind: &str, detail: &str| {
            report
                .evidence
                .push(json!({"path":path,"kind":kind,"detail":detail}))
        };
        match file {
            "project.pbxproj" => {
                apple = true;
                evidence(
                    "platform",
                    "Xcode project requires an Apple SDK/toolchain; Linux cannot build/run Apple platform targets.",
                );
                report.runtimes.push(
                    json!({"name":"Xcode / Apple SDK","source":path,"installationVerified":false}),
                );
            }
            "Package.swift" => {
                evidence(
                    "runtime",
                    "Swift package found; Linux compatibility depends on declared platforms and imported APIs.",
                );
                report.runtimes.push(json!({"name":"Swift","source":path}));
            }
            "package.json" => {
                portable = true;
                evidence(
                    "runtime",
                    "Node/JavaScript package manifest found; native dependencies may still impose platform restrictions.",
                );
                match serde_json::from_str::<Value>(content){
                    Ok(value)=>{
                        report.runtimes.push(json!({"name":"Node.js","source":path,"versionRequirement":value.pointer("/engines/node"),"packageManager":value.get("packageManager")}));
                        if let Some(scripts)=value.get("scripts").and_then(Value::as_object){for name in ["dev","start","build","test"] {if let Some(command)=scripts.get(name).and_then(Value::as_str){ report.suggested_commands.push(json!({"source":path,"script":name,"declaredCommand":command,"reviewRequired":true}));}}}
                        if let Some(os)=value.get("os").and_then(Value::as_array){if os.iter().any(|s|s=="!linux") || (!os.iter().any(|s|s=="linux") && os.iter().any(|s|s=="darwin")){report.warnings.push(format!("{path} declares OS restrictions: {os:?}."));apple=true;}}
                    },
                    Err(_)=>report.warnings.push(format!("{path} is invalid JSON or exceeds the 64 KiB inspection limit; details were not inferred.")),
                }
            }
            "Cargo.toml" | "pyproject.toml" | "requirements.txt" | "go.mod" | "Gemfile" => {
                portable = true;
                let runtime = match file {
                    "Cargo.toml" => "Rust",
                    "go.mod" => "Go",
                    "Gemfile" => "Ruby",
                    _ => "Python",
                };
                evidence(
                    "runtime",
                    "Language manifest found; native/platform dependencies still need verification.",
                );
                report.runtimes.push(json!({"name":runtime,"source":path}));
            }
            "compose.yaml" | "compose.yml" | "docker-compose.yaml" | "docker-compose.yml" => {
                evidence(
                    "services",
                    "Docker Engine and Compose are needed for the declared services. Service definitions/ports are not interpreted by this detector.",
                )
            }
            "Dockerfile" => evidence(
                "container",
                "Dockerfile found; Docker may be part of the intended workflow.",
            ),
            "devcontainer.json" => evidence(
                "environment",
                "Dev Container configuration found; image/features/lifecycle commands need review.",
            ),
            "flake.nix" | "shell.nix" => {
                evidence("environment", "Nix environment definition found.")
            }
            "mise.toml" | ".mise.toml" => evidence("environment", "mise tool configuration found."),
            ".nvmrc" | ".node-version" | ".python-version" => {
                evidence("version", "Runtime version hint found.");
                report.runtimes.push(json!({"name":if file==".python-version"{"Python"}else{"Node.js"},"versionHint":content.trim(),"source":path}));
            }
            _ => {}
        }
        if path == ".orvex/workspace.json" {
            match serde_json::from_str::<Value>(content) {
                Ok(value) => {
                    report.evidence.push(json!({"path":path,"kind":"environment","detail":"Explicit Orvex setup/services configuration found."}));
                    if let Some(commands) =
                        value.pointer("/setup/commands").and_then(Value::as_array)
                    {
                        for command in commands {
                            if let Some(command) = command.as_str() {
                                report.suggested_commands.push(json!({"source":path,"declaredCommand":command,"reviewRequired":true}));
                            }
                        }
                    }
                }
                Err(_) => report.warnings.push(format!(
                    "{path} could not be parsed; no setup commands inferred."
                )),
            }
        }
    }
    let configuration = crate::project_configuration::inspect(files);
    report.runtimes.extend(configuration.runtimes);
    report.services = configuration.services;
    report.configuration_keys = configuration.keys;
    report.needs.extend(configuration.needs);
    report.warnings.extend(configuration.warnings);
    if report.runtimes.iter().any(|runtime| {
        runtime["name"] == ".NET SDK"
            && runtime["targetFrameworks"]
                .as_array()
                .is_some_and(|targets| {
                    !targets.is_empty()
                        && targets.iter().all(|target| {
                            target.as_str().is_some_and(|target| {
                                target.starts_with("net") && !target.contains("windows")
                            })
                        })
                })
    }) {
        portable = true;
    }
    for runtime in &report.runtimes {
        report.needs.push(json!({"kind":"runtime","name":runtime["name"],"source":runtime["source"],"versionRequirement":runtime.get("versionRequirement").or(runtime.get("versionHint")),"status":"declared","installationVerified":false}));
    }
    for item in &report.evidence {
        let name = match item["kind"].as_str() {
            Some("services") => Some("Docker Engine and Compose"),
            Some("container") => Some("Docker Engine (if using the Dockerfile workflow)"),
            _ => None,
        };
        if let Some(name) = name {
            report.needs.push(json!({"kind":"tool","name":name,"source":item["path"],"status":"inferred","installationVerified":false}));
        }
    }
    for (path, content) in files {
        if path.rsplit('/').next() == Some("Dockerfile") {
            let definition = dockerfile(path, content);
            for image in definition["baseImages"].as_array().unwrap() {
                report.needs.push(json!({"kind":"containerImage","name":image,"source":path,"status":"declared","availabilityVerified":false}));
            }
            report.container_definitions.push(definition);
        }
    }
    report.linux_compatibility = match (apple, portable) {
        (true, false) => "unsupported",
        (true, true) => "mixed",
        (false, true) => "potentiallyCompatible",
        _ => "unknown",
    }
    .into();
    if apple {
        report.warnings.push("Apple platform requirements detected. The Linux VM can store/edit source but cannot provide Xcode or Apple SDKs. Mixed repositories may have separate Linux-compatible components.".into());
    }
    report
}

fn dockerfile(path: &str, content: &str) -> Value {
    let mut images = Vec::<String>::new();
    let mut stages = Vec::<String>::new();
    let mut build = Vec::<String>::new();
    let mut startup = Vec::<Value>::new();
    let mut variables = std::collections::BTreeSet::<String>::new();
    let mut ports = Vec::<String>::new();
    let logical = content.replace("\\\n", " ");
    for line in logical
        .lines()
        .map(str::trim)
        .filter(|s| !s.starts_with('#'))
    {
        let Some((instruction, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let value = value.trim();
        match instruction.to_ascii_uppercase().as_str() {
            "FROM"=>{
                let tokens=value.split_whitespace().collect::<Vec<_>>();
                if let Some(image)=tokens.iter().find(|token|!token.starts_with("--")) {
                    if *image != "scratch" && !stages.iter().any(|stage|stage.eq_ignore_ascii_case(image)) { images.push((*image).to_owned()); }
                }
                if let Some(i)=tokens.iter().position(|t|t.eq_ignore_ascii_case("as")) {if let Some(stage)=tokens.get(i+1){stages.push((*stage).into());}}
            },
            "RUN"=>build.push(value.into()),
            "CMD"|"ENTRYPOINT"=>startup.push(json!({"instruction":instruction.to_ascii_uppercase(),"declaration":value,"reviewRequired":true})),
            "ENV"|"ARG"=>{
                for (index,token) in value.split_whitespace().enumerate() {
                    if let Some((name,_))=token.split_once('=') { if !name.is_empty(){variables.insert(name.into());} }
                    else if index==0 {variables.insert(token.into());}
                }
            },
            "EXPOSE"=>ports.extend(value.split_whitespace().map(str::to_owned)),
            _=>{},
        }
    }
    json!({"source":path,"baseImages":images,"buildCommands":build,"startupInstructions":startup,
        "environmentVariableNames":variables,"exposedPorts":ports,"status":"declared","reviewRequired":true,
        "limitations":"Static Dockerfile declarations only. No shell evaluation, ARG expansion, package resolution, service discovery or installed-tool verification. ENV/ARG default values are omitted; listed names are not necessarily required user inputs."})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn docker_declarations_remain_structured_and_do_not_expose_env_defaults() {
        let mut files = BTreeMap::new();
        files.insert("Dockerfile".into(), "FROM node:22 AS build\nRUN npm ci\nFROM build AS final\nENV PORT=3000 TOKEN=private-value\nARG API_KEY\nEXPOSE 3000/tcp\nCMD [\"npm\",\"start\"]\n".into());
        let report = analyze("fixture".into(), &files);
        assert_eq!(
            report.container_definitions[0]["baseImages"],
            json!(["node:22"])
        );
        assert_eq!(
            report.container_definitions[0]["buildCommands"],
            json!(["npm ci"])
        );
        assert_eq!(
            report.container_definitions[0]["exposedPorts"],
            json!(["3000/tcp"])
        );
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("private-value")
        );
        assert!(report.needs.iter().any(|need| need["name"] == "node:22"));
        assert!(!report.automatic_setup);
    }
    #[test]
    fn detects_apple_node_and_mixed_projects() {
        let mut files = BTreeMap::new();
        files.insert("Patchly.xcodeproj/project.pbxproj".into(), String::new());
        assert_eq!(
            analyze("fixture".into(), &files).linux_compatibility,
            "unsupported"
        );
        files.insert(
            "web/package.json".into(),
            r#"{"engines":{"node":">=22"},"scripts":{"dev":"vite"}}"#.into(),
        );
        let result = analyze("fixture".into(), &files);
        assert_eq!(result.linux_compatibility, "mixed");
        assert_eq!(result.suggested_commands[0]["declaredCommand"], "vite");
        files.remove("Patchly.xcodeproj/project.pbxproj");
        assert_eq!(
            analyze("fixture".into(), &files).linux_compatibility,
            "potentiallyCompatible"
        );
    }
    #[test]
    fn does_not_invent_setup_for_unknown_or_invalid_manifests() {
        let mut files = BTreeMap::new();
        assert_eq!(
            analyze("fixture".into(), &files).linux_compatibility,
            "unknown"
        );
        files.insert("package.json".into(), "not json".into());
        let report = analyze("fixture".into(), &files);
        assert!(report.suggested_commands.is_empty());
        assert!(!report.automatic_setup);
    }
}
