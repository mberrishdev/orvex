//! Conservative configuration hints. Never return configuration/secret values.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn relevant(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.ends_with(".csproj")
        || file.ends_with(".fsproj")
        || file.ends_with(".cs")
        || file == "global.json"
        || file == "Directory.Packages.props"
        || file == "launchSettings.json"
        || ((file.starts_with("appsettings") || file.starts_with("appsecrets"))
            && file.ends_with(".json"))
        || file == ".env"
        || file.starts_with(".env.")
        || file.ends_with(".env.example")
}

#[derive(Default)]
pub(crate) struct Findings {
    pub(crate) needs: Vec<Value>,
    pub(crate) services: Vec<Value>,
    pub(crate) keys: Vec<Value>,
    pub(crate) runtimes: Vec<Value>,
    pub(crate) warnings: Vec<String>,
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 180
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_:.-".contains(&b))
}

fn service_hint(key: &str, value: &str) -> Option<&'static str> {
    let key = key.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    if key.contains("postgres")
        || key.contains("npgsql")
        || value.starts_with("postgres://")
        || value.starts_with("postgresql://")
        || (value.contains("host=") && value.contains("database="))
    {
        Some("PostgreSQL")
    } else if key.contains("redis")
        || value.starts_with("redis://")
        || value.starts_with("rediss://")
    {
        Some("Redis")
    } else if key.contains("rabbitmq")
        || value.starts_with("amqp://")
        || value.starts_with("amqps://")
    {
        Some("RabbitMQ")
    } else if key.contains("mongo")
        || value.starts_with("mongodb://")
        || value.starts_with("mongodb+srv://")
    {
        Some("MongoDB")
    } else if key.contains("sqlserver") {
        Some("SQL Server")
    } else if key.contains("mysql") {
        Some("MySQL")
    } else {
        None
    }
}

fn record_service(findings: &mut Findings, source: &str, key: &str, name: &str, status: &str) {
    let item = json!({"name":name,"source":source,"configurationKey":key,"status":status,"requiredAtRuntime":"unverified","reviewRequired":true,"availabilityVerified":false});
    if !findings.services.contains(&item) {
        findings.services.push(item.clone());
        findings.needs.push(json!({"kind":"service","name":name,"source":source,"configurationKey":key,"status":status,"reviewRequired":true,"availabilityVerified":false}));
    }
}

fn record_key(findings: &mut Findings, source: &str, key: &str, kind: &str) {
    if !safe_name(key) {
        return;
    }
    let item = json!({"name":key,"source":source,"kind":kind,"valueOmitted":true,"requiredAtRuntime":"unverified"});
    if !findings.keys.contains(&item) {
        findings.keys.push(item);
    }
}

fn walk_json(findings: &mut Findings, source: &str, path: &str, value: &Value) {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                if !safe_name(key) {
                    continue;
                }
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}:{key}")
                };
                walk_json(findings, source, &path, value);
            }
        }
        Value::Array(values) => {
            for value in values {
                walk_json(findings, source, path, value);
            }
        }
        _ => {
            record_key(findings, source, path, "configuration");
            if let Some(service) = service_hint(path, value.as_str().unwrap_or_default()) {
                record_service(findings, source, path, service, "configurationHint");
            }
        }
    }
}

fn quoted_arguments(content: &str, marker: &str) -> Vec<String> {
    content
        .split(marker)
        .skip(1)
        .filter_map(|part| {
            let part = part.trim_start();
            let part = part.strip_prefix('(')?.trim_start().strip_prefix('"')?;
            let name = part.split('"').next()?;
            safe_name(name).then(|| name.to_owned())
        })
        .take(64)
        .collect()
}

// appsettings allows comments/trailing commas. Strip syntax without touching
// quoted strings so URLs and passwords containing // remain intact.
fn json_config(content: &str) -> Result<Value, serde_json::Error> {
    let mut chars = content.trim_start_matches('\u{feff}').chars().peekable();
    let mut output = String::new();
    let (mut quoted, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if quoted {
            output.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        if c == '"' {
            quoted = true;
            output.push(c);
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for c in chars.by_ref() {
                if c == '\n' {
                    output.push('\n');
                    break;
                }
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
            continue;
        }
        output.push(c);
    }
    let chars = output.chars().collect::<Vec<_>>();
    let mut cleaned = String::new();
    let (mut quoted, mut escaped) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if !quoted
            && c == ','
            && chars[i + 1..]
                .iter()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| *c == '}' || *c == ']')
        {
            continue;
        }
        cleaned.push(c);
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        }
    }
    serde_json::from_str(&cleaned)
}

pub(crate) fn inspect(files: &BTreeMap<String, String>) -> Findings {
    let mut findings = Findings::default();
    for (path, content) in files {
        let file = path.rsplit('/').next().unwrap();
        if file.ends_with(".csproj") || file.ends_with(".fsproj") {
            let mut targets = BTreeSet::new();
            for tag in ["TargetFramework", "TargetFrameworks"] {
                if let Some(value) = content
                    .split(&format!("<{tag}>"))
                    .nth(1)
                    .and_then(|s| s.split(&format!("</{tag}>")).next())
                {
                    for target in value.split(';').map(str::trim).filter(|s| safe_name(s)) {
                        targets.insert(target.to_owned());
                    }
                }
            }
            findings.runtimes.push(json!({"name":".NET SDK","source":path,"targetFrameworks":targets,"installationVerified":false}));
            if content.contains("<UserSecretsId>") {
                findings.needs.push(json!({"kind":"externalConfiguration","name":".NET user secrets","source":path,"status":"referenced","valuesCopied":false,"reviewRequired":true}));
            }
        }
        if file.ends_with(".csproj")
            || file.ends_with(".fsproj")
            || file.ends_with(".cs")
            || file == "Directory.Packages.props"
        {
            for (marker, name) in [
                ("Npgsql", "PostgreSQL"),
                ("UseNpgsql", "PostgreSQL"),
                ("StackExchange.Redis", "Redis"),
                ("RabbitMQ.Client", "RabbitMQ"),
                ("MongoDB.Driver", "MongoDB"),
                ("UseSqlServer", "SQL Server"),
                ("Pomelo.EntityFrameworkCore.MySql", "MySQL"),
            ] {
                if content.contains(marker) {
                    record_service(&mut findings, path, marker, name, "providerReference");
                }
            }
        }
        if file.ends_with(".cs") {
            for marker in ["Configuration[", "configuration[", "_configuration["] {
                for part in content.split(marker).skip(1).take(64) {
                    if let Some(name) = part
                        .trim_start()
                        .strip_prefix('"')
                        .and_then(|s| s.split('"').next())
                        .filter(|s| safe_name(s))
                    {
                        record_key(&mut findings, path, name, "configurationReference");
                    }
                }
            }
            for key in quoted_arguments(content, "GetSection") {
                record_key(&mut findings, path, &key, "configurationSectionReference");
            }

            for key in quoted_arguments(content, "GetConnectionString") {
                record_key(
                    &mut findings,
                    path,
                    &format!("ConnectionStrings:{key}"),
                    "connectionStringReference",
                );
            }
            for key in quoted_arguments(content, "GetEnvironmentVariable") {
                record_key(&mut findings, path, &key, "environmentVariableReference");
                if let Some(name) = service_hint(&key, "") {
                    record_service(&mut findings, path, &key, name, "sourceReference");
                }
            }
        }
        if file == "global.json" {
            if let Ok(value) = json_config(content) {
                if let Some(version) = value
                    .pointer("/sdk/version")
                    .and_then(Value::as_str)
                    .filter(|s| safe_name(s))
                {
                    findings.runtimes.push(
                        json!({"name":".NET SDK","versionRequirement":version,"source":path}),
                    );
                }
            }
        }
        if ((file.starts_with("appsettings") || file.starts_with("appsecrets"))
            && file.ends_with(".json"))
            || file == "launchSettings.json"
        {
            match json_config(content) {
                Ok(value) => walk_json(&mut findings, path, "", &value),
                Err(_) => findings.warnings.push(format!(
                    "Could not parse {path}; configuration dependencies may be missing."
                )),
            }
        }
        if file == ".env" || file.starts_with(".env.") || file.ends_with(".env.example") {
            for line in content
                .lines()
                .map(str::trim)
                .filter(|s| !s.starts_with('#'))
            {
                if let Some((key, value)) =
                    line.strip_prefix("export ").unwrap_or(line).split_once('=')
                {
                    let key = key.trim();
                    record_key(&mut findings, path, key, "environmentVariable");
                    if let Some(name) = service_hint(key, value.trim().trim_matches(['\'', '"'])) {
                        record_service(&mut findings, path, key, name, "environmentHint");
                    }
                }
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dotnet_postgres_and_environment_keys_are_found_without_secret_values() {
        let files=BTreeMap::from([
            ("src/Api/Api.csproj".into(),"<Project><TargetFramework>net10.0</TargetFramework><PackageReference Include=\"Npgsql.EntityFrameworkCore.PostgreSQL\"/><UserSecretsId>secret-id</UserSecretsId></Project>".into()),
            ("src/Api/appsettings.Development.json".into(),"{ // local\n\"ConnectionStrings\":{\"Default\":\"Host=db;Database=app;Password=private-password\",},\"ApiKey\":\"private-api-key\",}".into()),
            (".env.example".into(),"POSTGRES_PASSWORD=private-env-value\nDATABASE_URL=postgresql://user:private-url-value@db/app".into()),
            ("src/Api/Program.cs".into(),"GetConnectionString(\"Default\"); Environment.GetEnvironmentVariable(\"CUSTOM_TOKEN\");".into()),
        ]);
        let result = inspect(&files);
        assert!(result.services.iter().any(|s| s["name"] == "PostgreSQL"));
        assert_eq!(result.runtimes[0]["name"], ".NET SDK");
        assert!(
            result
                .keys
                .iter()
                .any(|s| s["name"] == "ConnectionStrings:Default")
        );
        assert!(result.keys.iter().any(|s| s["name"] == "CUSTOM_TOKEN"));
        let serialized=json!({"services":result.services,"keys":result.keys,"needs":result.needs,"runtimes":result.runtimes}).to_string();
        for secret in [
            "private-password",
            "private-api-key",
            "private-env-value",
            "private-url-value",
            "secret-id",
        ] {
            assert!(!serialized.contains(secret));
        }
    }
}
