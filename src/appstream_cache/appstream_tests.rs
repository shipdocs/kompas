use super::*;
use std::time::Instant;

#[test]
fn bench_yaml_parsing() {
    // A small sample YAML to test structure
    let yaml_data = r#"
---
Origin: test-origin
MediaBaseUrl: https://example.com/media
---
Type: desktop-application
ID: org.example.App1
Name:
  C: App One
Summary:
  C: The first app
Icon:
  cached:
    - name: app1_icon.png
      width: 64
      height: 64
---
Type: desktop-application
ID: org.example.App2
Name:
  C: App Two
Summary:
  C: The second app
    "#;

    let idx = AppstreamCache::default();
    let start = Instant::now();
    // Note: this uses the new signature we are about to implement
    let result = idx.parse_yaml("test.yml", yaml_data.as_bytes());
    let duration = start.elapsed();

    assert!(result.is_ok());
    let (origin, infos, _) = result.unwrap();
    assert_eq!(origin, Some("test-origin".to_string()));
    assert_eq!(infos.len(), 2);

    println!("Parsed 2 YAML documents in {:?}", duration);
}

#[test]
fn bench_xml_parsing() {
    let xml_data = r#"<?xml version="1.0"?>
<components version="0.8" origin="test-origin">
  <component type="desktop-application">
    <id>org.example.App1</id>
    <name>App One</name>
    <summary>The first app</summary>
  </component>
  <component type="desktop-application">
    <id>org.example.App2</id>
    <name>App Two</name>
    <summary>The second app</summary>
  </component>
</components>
    "#;

    let idx = AppstreamCache::default();
    let start = Instant::now();
    // Note: this uses the new signature we are about to implement
    let result = idx.parse_xml("test.xml", xml_data.as_bytes());
    let duration = start.elapsed();

    assert!(result.is_ok());
    let (origin, infos, _) = result.unwrap();
    assert_eq!(origin, Some("test-origin".to_string()));
    assert_eq!(infos.len(), 2);

    println!("Parsed 2 XML components in {:?}", duration);
}

#[test]
fn malformed_translation_does_not_discard_the_repository() {
    let data = r#"---
Origin: zorin
---
Type: desktop-application
ID: org.example.First
Name: {C: First}
Summary: {C: First app}
---
Type: desktop-application
ID: org.example.Invalid
Name: {C: Invalid}
Keywords:
  ca_ES: [one]
  ca_ES: [two]
---
Type: desktop-application
ID: org.example.Last
Name: {C: Last}
Summary: {C: Last app}
"#;
    let (origin, infos, _) = AppstreamCache::default()
        .parse_yaml("zorin.yml", data.as_bytes())
        .unwrap();
    assert_eq!(origin.as_deref(), Some("zorin"));
    let ids: std::collections::HashSet<_> = infos.into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&AppId::new("org.example.First")));
    assert!(ids.contains(&AppId::new("org.example.Last")));
}

#[test]
fn invalid_repository_header_is_rejected() {
    assert!(
        AppstreamCache::default()
            .parse_yaml("invalid.yml", b"Origin: first\nOrigin: second\n")
            .is_err()
    );
}

#[test]
fn repeated_header_key_keeps_the_components() {
    // Zorin's extra catalog repeats a key in the header document.
    let yaml_data = r#"
---
File: DEP11
Version: '0.8'
File: DEP11
---
Type: desktop-application
ID: org.example.App1
Name:
  C: App One
Summary:
  C: The first app
"#;
    let (origin, infos, _) = AppstreamCache::default()
        .parse_yaml("test.yml", yaml_data.as_bytes())
        .expect("a repeated header key must not discard the catalog");
    assert_eq!(origin, None);
    assert_eq!(infos.len(), 1);
}

#[test]
fn non_catalog_yaml_is_still_rejected() {
    assert!(
        AppstreamCache::default()
            .parse_yaml("test.yml", b"- [unterminated")
            .is_err()
    );
}
