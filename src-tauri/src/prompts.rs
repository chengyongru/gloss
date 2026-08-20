use std::{fs, path::Path};

const TRIAGE_TEMPLATE: &str = include_str!("../prompts/triage.md");
const TRANSLATE_TEMPLATE: &str = include_str!("../prompts/translate.md");
const LEARNER_PROFILE_TEMPLATE: &str = include_str!("../prompts/learner-profile.md");

pub fn ensure_editable_templates(data_dir: &Path) -> Result<(), String> {
    let prompt_dir = data_dir.join("prompts");
    fs::create_dir_all(&prompt_dir)
        .map_err(|error| format!("Could not create the prompt folder: {error}"))?;
    seed_if_missing(&prompt_dir.join("triage.md"), TRIAGE_TEMPLATE)?;
    seed_if_missing(&prompt_dir.join("translate.md"), TRANSLATE_TEMPLATE)?;
    seed_if_missing(
        &prompt_dir.join("learner-profile.md"),
        LEARNER_PROFILE_TEMPLATE,
    )?;
    Ok(())
}

pub fn triage(data_dir: &Path, learner_profile: Option<&str>) -> Result<String, String> {
    let profile = learner_profile
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("No CEFR learning signals have been recorded yet.");
    Ok(load(data_dir, "triage.md", TRIAGE_TEMPLATE)?.replace("{{learner_profile}}", profile))
}

pub fn translate(data_dir: &Path) -> Result<String, String> {
    Ok(load(data_dir, "translate.md", TRANSLATE_TEMPLATE)?
        .trim()
        .to_owned())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_triage_template_has_profile_placeholder() {
        assert!(TRIAGE_TEMPLATE.contains("{{learner_profile}}"));
    }
}
