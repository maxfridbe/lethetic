use super::*;

fn write_skill(root: &Path, dir: &str, frontmatter: &str, body: &str) {
    let path = root.join(dir);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("SKILL.md"), format!("---\n{frontmatter}\n---\n{body}")).unwrap();
}

#[test]
fn lethetic_folders_win_and_claude_skills_start_disabled() {
    let lethetic = tempfile::tempdir().unwrap();
    let claude = tempfile::tempdir().unwrap();
    write_skill(lethetic.path(), "pdf", "name: pdf\ndescription: Lethetic's PDF skill", "Use pdfplumber.");
    write_skill(claude.path(), "pdf", "name: pdf\ndescription: Claude's PDF skill", "Other.");
    write_skill(
        claude.path(),
        "api",
        "name: claude-api\ndescription: |-\n  Build with the API.\n  Two lines.",
        "Body.",
    );
    write_skill(claude.path(), "broken", "name: broken", "No description.");
    let roots = vec![
        (lethetic.path().to_path_buf(), SkillSource::LetheticUser),
        (claude.path().to_path_buf(), SkillSource::ClaudeUser),
    ];

    let skills = discover_with(Path::new("."), &roots, &SkillSettings::default());
    let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, ["claude-api", "pdf"]);
    let pdf = &skills[1];
    assert_eq!(pdf.description, "Lethetic's PDF skill");
    assert!(pdf.enabled, "Lethetic skills are on by default");
    assert_eq!(skills[0].description, "Build with the API. Two lines.");
    assert!(!skills[0].enabled, "Claude skills wait to be enabled");

    let mut settings = SkillSettings::default();
    settings.enabled.insert("claude-api".to_string(), true);
    settings.enabled.insert("pdf".to_string(), false);
    let skills = discover_with(Path::new("."), &roots, &settings);
    assert!(skills[0].enabled && !skills[1].enabled, "menu choices override");
}

#[test]
fn the_model_gets_the_body_directory_and_file_list() {
    let root = tempfile::tempdir().unwrap();
    write_skill(root.path(), "pdf", "name: pdf\ndescription: PDFs", "# PDF\nRun scripts/fill.py");
    std::fs::create_dir_all(root.path().join("pdf/scripts")).unwrap();
    std::fs::write(root.path().join("pdf/scripts/fill.py"), "print(1)").unwrap();
    let skill = &discover_with(
        Path::new("."),
        &[(root.path().to_path_buf(), SkillSource::LetheticUser)],
        &SkillSettings::default(),
    )[0];
    let text = load_for_model(skill).unwrap();
    assert!(text.starts_with("# Skill: pdf\nDirectory: "));
    assert!(text.contains("scripts/fill.py"));
    assert!(text.ends_with("# PDF\nRun scripts/fill.py"));
    assert!(!text.contains("description: PDFs"), "frontmatter is not repeated");
}

#[test]
fn frontmatter_and_names_are_validated() {
    assert_eq!(
        split_frontmatter("---\nname: a\n---\nbody\n"),
        Some(("name: a", "body\n"))
    );
    assert_eq!(split_frontmatter("no frontmatter"), None);
    assert!(valid_name("skill-creator"));
    assert!(!valid_name("Bad Name"));
    assert!(!valid_name("../escape"));
}
