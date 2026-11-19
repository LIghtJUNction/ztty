use super::*;
use zgraphics::{Draw, DrawGeometry, MAX_PLACEHOLDER_RUNS, Placeholder};

#[path = "image_diacritics.rs"]
mod diacritics;
#[cfg(test)]
#[path = "image_cell_tests.rs"]
mod tests;

pub(super) struct Cache {
    key: (u64, u64, usize, usize, usize),
    draws: Vec<DrawGeometry>,
}

#[derive(Clone, Copy)]
struct Decoded {
    color: u32,
    placement: u32,
    image: u32,
    row: u32,
    column: u32,
}

impl Terminal {
    pub(super) fn image_draws(&self, history: usize) -> impl Iterator<Item = Draw<'_>> {
        let history = if self.state.modes.alternate_screen {
            0
        } else {
            history.min(self.history_len())
        };
        let graphics = &self.state.graphics;
        let virtual_placements = graphics.has_virtual_placements();
        if virtual_placements {
            let key = (
                self.text_revision,
                graphics.revision(),
                history,
                self.cols(),
                self.rows(),
            );
            let rebuild = self
                .image_cache
                .borrow()
                .as_ref()
                .is_none_or(|cache| cache.key != key);
            if rebuild {
                let cells = self.image_placeholders(history);
                let draws = graphics
                    .draws_with_placeholders(history, &cells)
                    .into_iter()
                    .map(|draw| draw.geometry())
                    .collect();
                *self.image_cache.borrow_mut() = Some(Box::new(Cache { key, draws }));
            }
        } else {
            self.image_cache.borrow_mut().take();
        }
        let cache = virtual_placements.then(|| self.image_cache.borrow());
        let mut direct = graphics.draws(history);
        let mut index = 0;
        core::iter::from_fn(move || {
            if let Some(cache) = &cache {
                let geometry = cache.as_ref()?.draws.get(index)?;
                index += 1;
                graphics.draw_geometry(geometry)
            } else {
                direct.next()
            }
        })
    }

    fn image_placeholders(&self, history: usize) -> Vec<Placeholder> {
        let mut runs: Vec<Placeholder> = Vec::new();
        for row in 0..self.rows() {
            let mut previous: Option<Decoded> = None;
            for col in 0..self.cols() {
                let cell = self.view_cell(col, row, history);
                if cell.ch != '\u{10eeee}' || cell.width == 0 {
                    previous = None;
                    continue;
                }
                let mut marks = self.cell_chars(cell).skip(1).filter_map(|ch| {
                    diacritics::VALUES
                        .binary_search(&(ch as u32))
                        .ok()
                        .map(|n| n as u32)
                });
                let image_row = marks.next();
                let image_col = marks.next();
                let upper = marks.next();
                if upper.is_some_and(|byte| byte > 255) {
                    previous = None;
                    continue;
                }
                let predecessor =
                    previous.filter(|p| p.color == cell.fg && p.placement == cell.link_id);
                let inherited = predecessor.filter(|p| image_row.is_none_or(|row| row == p.row));
                let decoded_row = image_row.unwrap_or_else(|| inherited.map_or(0, |p| p.row));
                let decoded_col = image_col
                    .unwrap_or_else(|| inherited.map_or(0, |p| p.column.saturating_add(1)));
                let inherited = inherited
                    .filter(|p| image_col.is_none_or(|col| col == p.column.saturating_add(1)));
                let upper = upper.unwrap_or_else(|| inherited.map_or(0, |p| p.image >> 24));
                let image = (cell.fg & 0xffffff) | (upper << 24);
                previous = Some(Decoded {
                    color: cell.fg,
                    placement: cell.link_id,
                    image,
                    row: decoded_row,
                    column: decoded_col,
                });
                if let Some(run) = runs.last_mut()
                    && run.image_id == image
                    && run.placement_id == cell.link_id
                    && run.row == row as i32
                    && run.col + run.columns as i32 == col as i32
                    && run.image_row == decoded_row
                    && run.image_col + run.columns == decoded_col
                {
                    run.columns += 1;
                } else {
                    if runs.len() >= MAX_PLACEHOLDER_RUNS {
                        return runs;
                    }
                    runs.push(Placeholder {
                        image_id: image,
                        placement_id: cell.link_id,
                        col: col as i32,
                        row: row as i32,
                        image_col: decoded_col,
                        image_row: decoded_row,
                        columns: 1,
                    });
                }
            }
        }
        runs
    }

    /// Install the validated platform I/O boundary on both terminal screens.
    pub fn set_graphics_reader(&mut self, reader: Option<zgraphics::ExternalReader>) {
        self.state.graphics.set_external_reader(reader);
        self.state.inactive_graphics.set_external_reader(reader);
    }

    /// Called by the host's existing event-loop clock, only scheduling visible
    /// images. No worker, polling thread or timer exists in the terminal core.
    pub fn advance_graphics(&mut self, now: u64, history: usize, visible: bool) -> Option<u64> {
        if !self.state.graphics.has_running_animations() {
            return None;
        }
        let mut ids = [0u32; 128];
        let mut count = 0;
        if visible {
            for draw in self.graphics_draws(history) {
                if count < ids.len() && !ids[..count].contains(&draw.image_id) {
                    ids[count] = draw.image_id;
                    count += 1;
                }
            }
        }
        let previous = self.state.graphics.revision();
        let next = self.state.graphics.advance_animations(now, &ids[..count]);
        if previous != self.state.graphics.revision() {
            self.state.screen.mark_all();
        }
        next
    }
}
