pub struct SlashCommandItem {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
}

pub struct SlashRegistry;

impl SlashRegistry {
    pub fn builtins() -> Self { SlashRegistry }
    pub fn slash_commands(&self) -> Vec<SlashCommandItem> { vec![] }
    pub fn builtin_commands(&self) -> Vec<(&str, &str, Vec<&str>)> { vec![] }
}
