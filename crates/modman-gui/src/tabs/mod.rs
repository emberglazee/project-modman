//! The GUI's tabs. Each one collects inputs and returns a [`Job`] when the user
//! asks for work; the shell runs it off-thread and shows the result.

pub mod audio;
pub mod build;
pub mod locres;
pub mod merge;
pub mod pak;
pub mod tools;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Tab {
    Merge,
    Build,
    Audio,
    Locres,
    Pak,
    Tools,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Merge => "Merge mods",
            Tab::Build => "Build presets",
            Tab::Audio => "Game audio",
            Tab::Locres => "Translations",
            Tab::Pak => "Pak files",
            Tab::Tools => "Scan & inspect",
        }
    }
}
