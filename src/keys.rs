//! A key of the Library window. The window reads the GPUI key name once, and the arrows and hjkl become the
//! four directions.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key<'a> {
    Left,
    Right,
    Up,
    Down,
    /// Any other key, by its GPUI name, for example `enter` or `p`.
    Other(&'a str),
}

impl<'a> Key<'a> {
    pub fn parse(name: &'a str) -> Self {
        match name {
            "h" | "left" => Key::Left,
            "l" | "right" => Key::Right,
            "k" | "up" => Key::Up,
            "j" | "down" => Key::Down,
            other => Key::Other(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Key;

    #[test]
    fn the_arrows_and_hjkl_are_the_four_directions() {
        let keys = ["h", "left", "l", "right", "k", "up", "j", "down", "enter"].map(Key::parse);
        use Key::*;
        assert_eq!(keys, [Left, Left, Right, Right, Up, Up, Down, Down, Other("enter")]);
    }
}
