use std::{fs, path::Path};

const SYSTEM_TEMPLATE: &str = include_str!("../prompts/system.md");
const TRIAGE_TEMPLATE: &str = include_str!("../prompts/triage.md");
const TRANSLATE_TEMPLATE: &str = include_str!("../prompts/translate.md");
const CORRECT_TEMPLATE: &str = include_str!("../prompts/correct.md");
const LEGACY_TRANSLATE_TEMPLATE_V4: &str = r#"<runtime_context>
Task: Translate the selected English text into natural, concise Simplified Chinese.

Selected text (JSON-encoded quoted material):
{{selected_text_json}}

Preserve names, product terms, links, and intentional tone. For the initial response, return one translation paragraph as plain Markdown text.
</runtime_context>
"#;
const EXPLAIN_SELECTION_TEMPLATE: &str = include_str!("../prompts/explain-selection.md");
const GOT_IT_TEMPLATE: &str = include_str!("../prompts/got-it.md");
const LEARNER_PROFILE_TEMPLATE: &str = include_str!("../prompts/learner-profile.md");
pub fn ensure_editable_templates(data_dir: &Path) -> Result<(), String> {
    let prompt_dir = data_dir.join("prompts");
    fs::create_dir_all(&prompt_dir)
        .map_err(|error| format!("Could not create the prompt folder: {error}"))?;
    seed_if_missing(&prompt_dir.join("system.md"), SYSTEM_TEMPLATE)?;
    seed_if_missing(&prompt_dir.join("triage.md"), TRIAGE_TEMPLATE)?;
    seed_if_missing(&prompt_dir.join("correct.md"), CORRECT_TEMPLATE)?;
    seed_or_migrate(
        &prompt_dir.join("translate.md"),
        TRANSLATE_TEMPLATE,
        LEGACY_TRANSLATE_TEMPLATE_V4,
    )?;
    seed_if_missing(
        &prompt_dir.join("explain-selection.md"),
        EXPLAIN_SELECTION_TEMPLATE,
    )?;
    seed_if_missing(&prompt_dir.join("got-it.md"), GOT_IT_TEMPLATE)?;
    seed_if_missing(
        &prompt_dir.join("learner-profile.md"),
        LEARNER_PROFILE_TEMPLATE,
    )?;
    Ok(())
}

pub fn system(data_dir: &Path, learner_profile: Option<&str>) -> Result<String, String> {
    let profile = learner_profile
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("No CEFR learning signals have been recorded yet.");
    Ok(load(data_dir, "system.md", SYSTEM_TEMPLATE)?.replace("{{learner_profile}}", profile))
}

pub fn triage_context(data_dir: &Path, selected_text: &str) -> Result<String, String> {
    runtime_context(data_dir, "triage.md", TRIAGE_TEMPLATE, selected_text)
}

pub fn translate_context(data_dir: &Path, selected_text: &str) -> Result<String, String> {
    runtime_context(data_dir, "translate.md", TRANSLATE_TEMPLATE, selected_text)
}

pub fn correct_context(data_dir: &Path, selected_text: &str) -> Result<String, String> {
    runtime_context(data_dir, "correct.md", CORRECT_TEMPLATE, selected_text)
}

pub fn explain_selection_context(data_dir: &Path, selected_text: &str) -> Result<String, String> {
    runtime_context(
        data_dir,
        "explain-selection.md",
        EXPLAIN_SELECTION_TEMPLATE,
        selected_text,
    )
}

pub fn got_it_context(data_dir: &Path, selected_text: &str) -> Result<String, String> {
    runtime_context(data_dir, "got-it.md", GOT_IT_TEMPLATE, selected_text)
}

pub fn learner_profile(data_dir: &Path) -> Result<String, String> {
    Ok(
        load(data_dir, "learner-profile.md", LEARNER_PROFILE_TEMPLATE)?
            .trim()
            .to_owned(),
    )
}

fn load(data_dir: &Path, filename: &str, fallback: &str) -> Result<String, String> {
    let path = data_dir.join("prompts").join(filename);
    match fs::read_to_string(&path) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) => Err(format!("The prompt template {filename} is empty.")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(fallback.to_owned()),
        Err(error) => Err(format!(
            "Could not read the prompt template {filename}: {error}"
        )),
    }
}

fn seed_if_missing(path: &Path, contents: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, contents)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))
}

fn seed_or_migrate(path: &Path, contents: &str, legacy: &str) -> Result<(), String> {
    match fs::read_to_string(path) {
        Ok(existing) if templates_match(&existing, legacy) => fs::write(path, contents)
            .map_err(|error| format!("Could not update {}: {error}", path.display())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::write(path, contents)
            .map_err(|error| format!("Could not create {}: {error}", path.display())),
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

fn templates_match(left: &str, right: &str) -> bool {
    left.replace("\r\n", "\n").trim_end() == right.replace("\r\n", "\n").trim_end()
}

fn runtime_context(
    data_dir: &Path,
    filename: &str,
    fallback: &str,
    selected_text: &str,
) -> Result<String, String> {
    let template = load(data_dir, filename, fallback)?;
    if !template.contains("{{selected_text_json}}") {
        return Err(format!(
            "The prompt template {filename} is missing {{{{selected_text_json}}}}."
        ));
    }
    let selected_text_json = serde_json::to_string(selected_text)
        .map_err(|error| format!("Could not encode the selected text: {error}"))?;
    Ok(template.replace("{{selected_text_json}}", &selected_text_json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_templates_separate_soul_from_runtime_context() {
        assert!(SYSTEM_TEMPLATE.contains("{{learner_profile}}"));
        assert!(!SYSTEM_TEMPLATE.contains("{{selected_text_json}}"));
        assert!(TRIAGE_TEMPLATE.contains("{{selected_text_json}}"));
        assert!(TRANSLATE_TEMPLATE.contains("{{selected_text_json}}"));
        assert!(CORRECT_TEMPLATE.contains("{{selected_text_json}}"));
        assert!(EXPLAIN_SELECTION_TEMPLATE.contains("{{selected_text_json}}"));
        assert!(GOT_IT_TEMPLATE.contains("{{selected_text_json}}"));
    }

    #[test]
    fn translate_template_is_bidirectional_and_adaptive() {
        assert!(TRANSLATE_TEMPLATE.contains("predominantly Chinese"));
        assert!(TRANSLATE_TEMPLATE.contains("predominantly English"));
        assert!(TRANSLATE_TEMPLATE.contains("Never translate into a third language"));
    }

    #[test]
    fn correct_template_preserves_voice_and_explains_naturalness() {
        assert!(CORRECT_TEMPLATE.contains("smallest changes"));
        assert!(CORRECT_TEMPLATE.contains("understandable but unnatural"));
        assert!(CORRECT_TEMPLATE.contains("stylistic preference as a rule"));
    }

    #[test]
    fn untouched_legacy_translate_template_is_migrated() {
        let data_dir =
            std::env::temp_dir().join(format!("gloss-prompts-test-{}", uuid::Uuid::new_v4()));
        let prompt_dir = data_dir.join("prompts");
        fs::create_dir_all(&prompt_dir).unwrap();
        fs::write(
            prompt_dir.join("translate.md"),
            LEGACY_TRANSLATE_TEMPLATE_V4,
        )
        .unwrap();

        ensure_editable_templates(&data_dir).unwrap();

        let migrated = fs::read_to_string(prompt_dir.join("translate.md")).unwrap();
        fs::remove_dir_all(data_dir).unwrap();
        assert_eq!(migrated, TRANSLATE_TEMPLATE);
    }

    #[test]
    fn customized_translate_template_is_preserved() {
        let data_dir =
            std::env::temp_dir().join(format!("gloss-prompts-test-{}", uuid::Uuid::new_v4()));
        let prompt_dir = data_dir.join("prompts");
        fs::create_dir_all(&prompt_dir).unwrap();
        let custom = "Custom {{selected_text_json}} translation prompt";
        fs::write(prompt_dir.join("translate.md"), custom).unwrap();

        ensure_editable_templates(&data_dir).unwrap();

        let retained = fs::read_to_string(prompt_dir.join("translate.md")).unwrap();
        fs::remove_dir_all(data_dir).unwrap();
        assert_eq!(retained, custom);
    }
}
