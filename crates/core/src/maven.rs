//! Maven coordinates and metadata.

use anyhow::{bail, Result};

/// `group:artifact:version[:classifier][@ext]` → relative path in a maven repo.
pub fn coord_to_path(coord: &str) -> Result<String> {
    let (coord, ext) = match coord.split_once('@') {
        Some((c, e)) => (c, e),
        None => (coord, "jar"),
    };
    let parts: Vec<&str> = coord.split(':').collect();
    if parts.len() < 3 {
        bail!("bad maven coordinate {coord:?}");
    }
    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let file = match parts.get(3) {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    Ok(format!("{group}/{artifact}/{version}/{file}"))
}

/// Identity of a library ignoring its version: `group:artifact[:classifier]`.
pub fn coord_key(coord: &str) -> String {
    let coord = coord.split('@').next().unwrap_or(coord);
    let parts: Vec<&str> = coord.split(':').collect();
    match parts.len() {
        0..=2 => coord.to_string(),
        3 => format!("{}:{}", parts[0], parts[1]),
        _ => format!("{}:{}:{}", parts[0], parts[1], parts[3]),
    }
}

/// Pull every `<version>` out of a maven-metadata.xml (in file order).
pub fn metadata_versions(xml: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        rest = &rest[start + 9..];
        if let Some(end) = rest.find("</version>") {
            out.push(rest[..end].trim().to_string());
            rest = &rest[end..];
        } else {
            break;
        }
    }
    out
}

/// Compare dotted version strings numerically where possible ("1.21.10" > "1.21.9").
pub fn cmp_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<(u64, String)> {
        s.split(|c: char| c == '.' || c == '-' || c == '+')
            .map(|p| (p.parse::<u64>().unwrap_or(0), p.to_string()))
            .collect()
    };
    let (pa, pb) = (parse(a), parse(b));
    for (x, y) in pa.iter().zip(pb.iter()) {
        let o = x.0.cmp(&y.0).then_with(|| x.1.cmp(&y.1));
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    pa.len().cmp(&pb.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(
            coord_to_path("org.lwjgl:lwjgl:3.3.3").unwrap(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar"
        );
        assert_eq!(
            coord_to_path("org.lwjgl:lwjgl:3.3.3:natives-windows").unwrap(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"
        );
        assert_eq!(
            coord_to_path("a.b:c:1@zip").unwrap(),
            "a/b/c/1/c-1.zip"
        );
    }

    #[test]
    fn keys() {
        assert_eq!(coord_key("org.ow2.asm:asm:9.6"), "org.ow2.asm:asm");
        assert_eq!(
            coord_key("org.lwjgl:lwjgl:3.3.3:natives-windows"),
            "org.lwjgl:lwjgl:natives-windows"
        );
    }

    #[test]
    fn metadata() {
        let xml = "<metadata><versioning><versions><version>1.0</version><version>1.1</version></versions></versioning></metadata>";
        assert_eq!(metadata_versions(xml), vec!["1.0", "1.1"]);
    }

    #[test]
    fn ordering() {
        use std::cmp::Ordering::*;
        assert_eq!(cmp_versions("1.21.10", "1.21.9"), Greater);
        assert_eq!(cmp_versions("2.0.110", "2.0.99"), Greater);
        assert_eq!(cmp_versions("1.0", "1.0"), Equal);
    }
}
