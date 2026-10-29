//! History rows packed small while idle: text, style runs, extras.

use std::iter;
use std::sync::Arc;

use crate::grid::{PackedRow, Row};
use crate::term::cell::{Cell, CellExtra, Flags};
use crate::vte::ansi::Color;

/// Cells in a row sharing one style.
struct Run {
    len: usize,
    fg: Color,
    bg: Color,
    flags: Flags,
}

/// A row as its characters, its style runs and its extras, the same `Arc`s.
struct Packed {
    text: Box<str>,
    runs: Box<[Run]>,
    extras: Box<[(usize, Arc<CellExtra>)]>,
    /// The cell the row ends in, repeated.
    tail: Option<Cell>,
    tail_len: usize,
    occ: usize,
}

/// Whether two cells are one, their extras the same allocation.
fn same(a: &Cell, b: &Cell) -> bool {
    let extras = match (&a.extra, &b.extra) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    };
    extras && a.c == b.c && a.fg == b.fg && a.bg == b.bg && a.flags == b.flags
}

/// Pack a row of cells.
pub fn pack(row: &Row<Cell>) -> Box<dyn PackedRow<Cell>> {
    let cells = &row[..];
    let mut body = cells.len();
    if let Some(last) = cells.last() {
        while body > 0 && same(&cells[body - 1], last) {
            body -= 1;
        }
    }
    let mut runs: Vec<Run> = Vec::new();
    for cell in &cells[..body] {
        match runs.last_mut() {
            Some(run) if run.fg == cell.fg && run.bg == cell.bg && run.flags == cell.flags => {
                run.len += 1
            },
            _ => runs.push(Run { len: 1, fg: cell.fg, bg: cell.bg, flags: cell.flags }),
        }
    }
    let extras = cells[..body]
        .iter()
        .enumerate()
        .filter_map(|(column, cell)| Some((column, cell.extra.clone()?)))
        .collect();
    Box::new(Packed {
        text: cells[..body].iter().map(|cell| cell.c).collect(),
        runs: runs.into(),
        extras,
        tail: cells.last().cloned(),
        tail_len: cells.len() - body,
        occ: row.occ,
    })
}

impl PackedRow<Cell> for Packed {
    fn unpack(&self) -> Row<Cell> {
        let mut chars = self.text.chars();
        let mut cells = Vec::with_capacity(self.text.len() + self.tail_len);
        for run in &self.runs {
            for _ in 0..run.len {
                let c = chars.next().expect("a packed row's text covers its runs");
                cells.push(Cell { c, fg: run.fg, bg: run.bg, flags: run.flags, extra: None });
            }
        }
        for (column, extra) in &self.extras {
            cells[*column].extra = Some(Arc::clone(extra));
        }
        cells.extend(iter::repeat_n(self.tail.clone(), self.tail_len).flatten());
        Row::from_vec(cells, self.occ)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Column;
    use crate::vte::ansi::{NamedColor, Rgb};

    #[test]
    fn a_row_unpacks_as_it_was_its_extras_the_same() {
        let mut row = Row::<Cell>::new(12);
        let text = "ab中 x";
        for (column, c) in text.chars().enumerate() {
            row[Column(column)].c = c;
        }
        row[Column(2)].flags = Flags::WIDE_CHAR;
        row[Column(3)].flags = Flags::WIDE_CHAR_SPACER;
        row[Column(1)].fg = Color::Spec(Rgb { r: 1, g: 2, b: 3 });
        row[Column(5)].bg = Color::Indexed(9);
        row[Column(0)].push_zerowidth('\u{301}');
        row[Column(5)].push_zerowidth('\u{302}');
        // Trailing cells share one extra; their identity stays.
        let shared = Arc::new(CellExtra::default());
        for column in 8..12 {
            row[Column(column)].fg = Color::Named(NamedColor::Red);
            row[Column(column)].extra = Some(Arc::clone(&shared));
        }
        row.occ = 12;
        let packed = pack(&row);
        let back = packed.unpack();
        assert_eq!(back, row);
        assert_eq!(back.occ, row.occ);
        for column in 0..12 {
            match (&back[Column(column)].extra, &row[Column(column)].extra) {
                (Some(a), Some(b)) => assert!(Arc::ptr_eq(a, b), "column {column}"),
                (None, None) => {},
                _ => panic!("column {column}'s extra"),
            }
        }
        // Equal extras in distinct allocations stay distinct.
        let mut apart = Row::<Cell>::new(3);
        for column in 0..3 {
            apart[Column(column)].extra = Some(Arc::new(CellExtra::default()));
        }
        let back = pack(&apart).unpack();
        let (first, last) = (back[Column(0)].extra.clone(), back[Column(2)].extra.clone());
        assert!(!Arc::ptr_eq(&first.unwrap(), &last.unwrap()));
    }

    /// Every cell of every line, history first.
    fn lines(term: &crate::Term<crate::event::VoidListener>) -> Vec<Vec<Cell>> {
        use crate::grid::Dimensions;
        let grid = term.grid();
        let rows = grid.topmost_line().0..=grid.bottommost_line().0;
        rows.map(|line| grid[crate::index::Line(line)][..].to_vec()).collect()
    }

    #[test]
    fn compacting_history_changes_nothing_a_terminal_shows() {
        use crate::event::VoidListener as Void;
        use crate::term::Config;
        use crate::term::test::TermSize;
        use crate::vte::ansi::Processor;

        let config = Config { scrolling_history: 200, ..Config::default() };
        let size = TermSize::new(20, 6);
        let (mut plain, mut packed) =
            (crate::Term::new(config.clone(), &size, Void), crate::Term::new(config, &size, Void));
        let mut parser: (Processor, Processor) = (Processor::new(), Processor::new());
        let mut output = String::new();
        for n in 0..400 {
            let styled = match n % 5 {
                0 => format!("\x1b[3{}m{n} red\x1b[0m", n % 8),
                1 => format!("中文{n} e\u{301}"),
                2 => format!("\x1b]8;id={n};https://x/{n}\x1b\\link{n}\x1b]8;;\x1b\\"),
                3 => format!("\x1b[4:3m\x1b[58:2::1:2:3m{n} under\x1b[0m"),
                _ => format!("\x1b7\x1b[2;5r\x1b[5;1H{n} region\n\x1b[r\x1b8"),
            };
            output.push_str(&styled);
            output.push_str("\r\n");
        }
        for (step, chunk) in output.as_bytes().chunks(997).enumerate() {
            parser.0.advance(&mut plain, chunk);
            parser.1.advance(&mut packed, chunk);
            if step % 3 == 0 {
                packed.compact();
            }
            assert_eq!(lines(&plain), lines(&packed), "after chunk {step}");
        }
        use crate::grid::Dimensions;
        assert_eq!(packed.grid().history_size(), 200, "history to pack");
        packed.compact();
        // Reading unpacks; the next compaction packs again.
        assert_eq!(lines(&plain), lines(&packed));
        packed.compact();
        plain.resize(TermSize::new(13, 9));
        packed.resize(TermSize::new(13, 9));
        assert_eq!(lines(&plain), lines(&packed), "after a resize");
    }
}
