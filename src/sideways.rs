//! A row of cards of one width that scrolls sideways. The row builds only the cards near its visible part, so a
//! row of 150 cast cards costs as little to draw as a row of 10. Padding at each end stands in for the cards that
//! it does not build, so the scroll width stays the same.

use std::ops::Range;

use gpui::{
    AnyElement, Div, ElementId, InteractiveElement as _, ParentElement, ScrollHandle, Stateful,
    StatefulInteractiveElement as _, Styled, div, point, px,
};

/// The cards that the row builds on each side of the visible cards. A wheel step moves less than this.
const EXTRA: usize = 2;

/// The size of a row: the card width, the gap between the cards, and the padding at each end.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub width: f32,
    pub gap: f32,
    pub pad: f32,
}

impl Row {
    /// The indexes of the cards to build, of `count` cards, when the row is scrolled `scrolled` pixels to the
    /// right and shows `visible` pixels of it.
    pub fn range(self, count: usize, scrolled: f32, visible: f32) -> Range<usize> {
        let step = self.width + self.gap;
        let first = ((scrolled - self.pad) / step).floor().max(0.) as usize;
        let end = ((scrolled - self.pad + visible) / step).ceil().max(0.) as usize;
        first.saturating_sub(EXTRA).min(count)..(end + EXTRA).min(count)
    }

    /// The row of `count` cards, with the id `id` and the scroll `scroll`. `viewport` is the width of the window,
    /// which the row uses before it has its own width. `card` builds the card at an index.
    pub fn build(
        self,
        id: impl Into<ElementId>,
        scroll: &ScrollHandle,
        count: usize,
        viewport: f32,
        card: impl Fn(usize) -> AnyElement,
    ) -> Stateful<Div> {
        let shown = f32::from(scroll.bounds().size.width);
        let range = self.range(count, -f32::from(scroll.offset().x), if shown > 0. { shown } else { viewport });
        let step = self.width + self.gap;
        div()
            .id(id)
            .flex()
            .gap(px(self.gap))
            .pl(px(self.pad + range.start as f32 * step))
            .pr(px(self.pad + (count - range.end) as f32 * step))
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .track_scroll(scroll)
            .children(range.map(card))
    }

    /// Scrolls `scroll` the least amount that shows the whole card at `index`.
    pub fn reveal(self, scroll: &ScrollHandle, index: usize) {
        let shown = f32::from(scroll.bounds().size.width);
        let scrolled = -f32::from(scroll.offset().x);
        let left = self.pad + index as f32 * (self.width + self.gap);
        let to = if left < scrolled {
            left - self.pad
        } else if left + self.width > scrolled + shown {
            left + self.width + self.pad - shown
        } else {
            return;
        };
        scroll.set_offset(point(px(-to.max(0.)), px(0.)));
    }
}

#[cfg(test)]
mod tests {
    use super::Row;

    const CAST: Row = Row { width: 110., gap: 14., pad: 0. };

    #[test]
    fn a_long_row_builds_only_the_cards_near_the_visible_part() {
        // 1400 pixels show about 12 cards of 124 pixels.
        assert_eq!(CAST.range(150, 0., 1400.), 0..14);
        assert_eq!(CAST.range(150, 124. * 50., 1400.), 48..64);
        assert_eq!(CAST.range(150, 124. * 145., 1400.), 143..150, "the end of the row");
        assert_eq!(CAST.range(5, 0., 1400.), 0..5, "a short row builds each card");
        assert_eq!(CAST.range(0, 0., 1400.), 0..0);
    }

    #[test]
    fn the_padding_of_the_row_comes_before_the_first_card() {
        let seasons = Row { width: 130., gap: 14., pad: 48. };
        assert_eq!(seasons.range(40, 0., 300.), 0..4);
        assert_eq!(seasons.range(40, 48. + 144. * 10., 300.), 8..15);
    }
}
