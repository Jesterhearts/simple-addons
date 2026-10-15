//! Reading the header of a `.toc` file, which is how an addon describes
//! itself to the game.
//!
//! The header is a run of `## Key: Value` lines at the top of the file.
//! Everything else in the file is a list of source files, which the installer
//! does not care about.

use std::fs;
use std::path::Path;

/// What an addon says about itself. Every field is optional, because the game
/// treats them all as optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toc {
    pub title: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub notes: Option<String>,
    /// The game builds the addon declares support for, newest listed first by
    /// convention. An addon that works across game versions lists several.
    pub interface: Vec<u32>,
    /// The addons that have to be installed for this one to load, by folder
    /// name.
    pub dependencies: Vec<String>,
}

/// Reads a toc file.
///
/// An unreadable file still gives usable metadata, because what matters for
/// installing is the folder, not the description inside it.
pub fn read(path: &Path) -> Toc {
    match fs::read(path) {
        Ok(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => Toc::default(),
    }
}

pub fn parse(text: &str) -> Toc {
    // Some editors start a file with a byte order mark, which would make the
    // first line look like something other than a directive.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut toc = Toc::default();
    for (key, value) in directives(text) {
        match key.to_ascii_lowercase().as_str() {
            "title" => toc.title = Some(strip_ui_escapes(value)),
            "version" => toc.version = Some(value.to_string()),
            "author" => toc.author = Some(strip_ui_escapes(value)),
            "notes" => toc.notes = Some(strip_ui_escapes(value)),
            "interface" => toc.interface = parse_interface(value),
            key if names_dependencies(key) => toc.dependencies.extend(parse_addon_list(value)),
            _ => {}
        }
    }
    toc
}

/// The `## Key: Value` lines of a toc file, in the order they appear.
fn directives(text: &str) -> Vec<(&str, &str)> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("##"))
        .filter_map(|directive| directive.split_once(':'))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect()
}

/// `## Interface: 110107, 110200` means the addon supports both builds.
fn parse_interface(value: &str) -> Vec<u32> {
    value
        .split(',')
        .filter_map(|build| build.trim().parse().ok())
        .collect()
}

/// The game reads `## Dependencies`, `## RequiredDeps` and any directive whose
/// name starts with `Dep` as the same list, and adds a repeated directive to
/// it. `## OptionalDeps` names addons that are not required, so it is not
/// part of the list. `key` is in lower case.
fn names_dependencies(key: &str) -> bool {
    key == "requireddeps" || key.starts_with("dep")
}

/// `## Dependencies: Details, WeakAuras` names two addons.
///
/// An inline directive such as `[AllowLoadGameType classic]` limits a line to
/// some game versions. It is not a name, and it can hold commas of its own,
/// so it is removed before the names are split. The dependency is kept for
/// every game version: where these names lead to an addon being removed, it
/// is named to the user first, so a name too many shows before anything
/// happens, while a name too few would hide an addon that is about to break.
fn parse_addon_list(value: &str) -> Vec<String> {
    strip_inline_directives(value)
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// Removes every `[...]` part of a directive value.
fn strip_inline_directives(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut depth = 0usize;
    for character in text.chars() {
        match character {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(character),
            _ => {}
        }
    }
    plain
}

/// Removes the game's inline markup so that a title reads as plain text.
///
/// The game accepts colour codes (`|cff00ff00` ... `|r`), inline textures
/// (`|Tpath:size|t`) and line breaks (`|n`) inside these fields, and addons
/// use them freely in their titles.
pub fn strip_ui_escapes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '|' {
            plain.push(character);
            continue;
        }
        match characters.next() {
            Some('c') => skip(&mut characters, 8),
            Some('r') => {}
            Some('n') => plain.push(' '),
            Some('T') => skip_texture(&mut characters),
            Some('|') => plain.push('|'),
            Some(other) => plain.push(other),
            None => {}
        }
    }
    plain.trim().to_string()
}

fn skip(characters: &mut impl Iterator<Item = char>, count: usize) {
    for _ in 0..count {
        characters.next();
    }
}

/// An inline texture runs to its closing `|t`, which is dropped with it.
fn skip_texture(characters: &mut impl Iterator<Item = char>) {
    for character in characters.by_ref() {
        if character == '|' {
            characters.next();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_header_and_ignores_the_file_list() {
        let toc = parse(
            "## Interface: 110107, 110200\n\
             ## Title: My Addon\n\
             ## Version: 1.2.3\n\
             ## Author: Someone\n\
             ## Notes: Does a thing.\n\
             \n\
             Core.lua\n\
             Modules\\Thing.lua\n",
        );

        assert_eq!(
            toc,
            Toc {
                title: Some("My Addon".to_string()),
                version: Some("1.2.3".to_string()),
                author: Some("Someone".to_string()),
                notes: Some("Does a thing.".to_string()),
                interface: vec![110107, 110200],
                dependencies: Vec::new(),
            }
        );
    }

    #[test]
    fn a_file_without_a_header_gives_nothing() {
        assert_eq!(parse("Core.lua\n"), Toc::default());
    }

    #[test]
    fn unknown_and_malformed_directives_are_skipped() {
        let toc = parse("## X-Website: https://example.invalid\n## Title\n## Version: 2\n");

        assert_eq!(toc.version, Some("2".to_string()));
        assert_eq!(toc.title, None);
    }

    #[test]
    fn keys_are_matched_whatever_their_case() {
        assert_eq!(parse("## TITLE: Shout\n").title, Some("Shout".to_string()));
    }

    #[test]
    fn titles_lose_their_markup() {
        assert_eq!(
            strip_ui_escapes("|cff33ff99Deadly|r Boss Mods"),
            "Deadly Boss Mods"
        );
        assert_eq!(
            strip_ui_escapes("|TInterface\\Icons\\icon:16|t Plater"),
            "Plater"
        );
        assert_eq!(strip_ui_escapes("First|nSecond"), "First Second");
        assert_eq!(strip_ui_escapes("Pipes || here"), "Pipes | here");
        assert_eq!(strip_ui_escapes("Plain"), "Plain");
    }

    #[test]
    fn a_truncated_escape_does_not_lose_the_rest() {
        assert_eq!(strip_ui_escapes("Name|"), "Name");
        assert_eq!(strip_ui_escapes("|cff00"), "");
    }

    #[test]
    fn reads_every_way_of_naming_a_required_dependency() {
        let toc = parse(
            "## Dependencies: WeakAuras, Details\n\
             ## RequiredDeps: LibStub\n\
             ## Dep: Ace3\n\
             ## DEPS: Plater\n",
        );

        assert_eq!(
            toc.dependencies,
            vec!["WeakAuras", "Details", "LibStub", "Ace3", "Plater"]
        );
    }

    #[test]
    fn optional_dependencies_are_not_required() {
        let toc = parse("## OptionalDeps: Masque, LibSharedMedia-3.0\n");

        assert!(toc.dependencies.is_empty());
    }

    #[test]
    fn inline_directives_are_not_dependency_names() {
        let toc = parse(
            "## Dependencies: Questie [AllowLoadGameType vanilla, tbc], Details\n\
             ## Dependencies: , [ExcludeLoadGameType standard]\n",
        );

        assert_eq!(toc.dependencies, vec!["Questie", "Details"]);
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_directive() {
        let toc = parse("\u{feff}## Dependencies: Foo\n## Title: Bar\n");

        assert_eq!(toc.dependencies, vec!["Foo"]);
        assert_eq!(toc.title, Some("Bar".to_string()));
    }

    #[test]
    fn a_non_numeric_interface_is_left_out() {
        assert_eq!(
            parse("## Interface: 110107, oops\n").interface,
            vec![110107]
        );
    }
}
