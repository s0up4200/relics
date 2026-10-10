# Prototype: library window look and layout

This is throwaway code for the ticket "Library window look and layout" (s0up4200/silo-omarchy#10). Do not merge it.

One GPUI window shows three layouts of the library window. The data is fake, and the app sends no request to a server. Each poster is a colored placeholder with the title on it.

- A, Web clone: this layout follows the Silo web client. It has a left sidebar, a featured hero and rows of cards. Continue watching uses 16:9 cards, and the other rows use 2:3 posters. A library page shows a poster grid, and the detail view is a full page with a backdrop. Cards are round and buttons are pills.
- B, Split pane: tabs are at the top. A list is on the left, and a detail pane on the right shows the selected item. The page never changes. Controls are square, in the Omarchy style.
- C, Grid sheet: a filter field and scope chips are at the top. Below them are a Continue strip and one poster grid. The detail view opens as a sheet over the grid. Controls are square, in the Omarchy style.

gpui-omarchy supplies the colors from the live Omarchy theme. The window app id is `silo-omarchy`.

## Run

Run these commands in `prototypes/library-layout`:

```sh
cargo run          # starts on A
cargo run -- b     # starts on B
cargo run -- c     # starts on C
```

Press F1, F2 or F3 to change the layout. The yellow bar at the bottom is the prototype switcher. It is not part of the design. It shows the keys of the current layout and the result of the last key.
