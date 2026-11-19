//! Relative anchors and text-backed placements share the renderer contract.
use alloc::vec::Vec;

use crate::{Draw, Graphics, PixelRect, Placement, clamp_i32, protocol::Command};

const MAX_PARENT_DEPTH: usize = 16;
pub const MAX_PLACEHOLDER_RUNS: usize = 8192;

/// A decoded placeholder cell. `col,row` locate the text cell in the visible
/// viewport; `image_col,image_row` locate its tile inside the virtual image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placeholder {
    pub image_id: u32,
    pub placement_id: u32,
    pub col: i32,
    pub row: i32,
    pub image_col: u32,
    pub image_row: u32,
    /// Adjacent text cells continuing the same image row/transform.
    pub columns: u32,
}

impl Graphics {
    pub fn has_virtual_placements(&self) -> bool {
        self.placements.iter().any(|p| p.virtual_placement)
    }

    pub(crate) fn validate_parent(&self, image_id: u32, c: &Command) -> Result<(), &'static str> {
        if c.virtual_placement && c.parent_image != 0 {
            return Err("EINVAL:a virtual placement cannot be relative");
        }
        if c.parent_image == 0 {
            if c.parent_placement != 0 || c.horizontal != 0 || c.vertical != 0 {
                return Err("EINVAL:relative offsets need a parent");
            }
            return Ok(());
        }
        let mut parent = Some((c.parent_image, c.parent_placement));
        let mut depth = 0;
        for _ in 0..MAX_PARENT_DEPTH {
            let Some(key) = parent else { break };
            depth += 1;
            if key == (image_id, c.placement) {
                return Err("ECYCLE:relative placement cycle");
            }
            let p = self
                .placements
                .iter()
                .find(|p| (p.image_id, p.id) == key)
                .ok_or("ENOPARENT:parent placement not found")?;
            parent = p.parent;
        }
        if parent.is_some() {
            return Err("ETOODEEP:relative placement depth exceeded");
        }
        // Replacing an existing parent can lengthen every descendant chain.
        for child in &self.placements {
            let mut next = child.parent;
            for distance in 1..=MAX_PARENT_DEPTH {
                let Some(key) = next else { break };
                if key == (image_id, c.placement) {
                    if depth + distance > MAX_PARENT_DEPTH {
                        return Err("ETOODEEP:relative descendant depth exceeded");
                    }
                    break;
                }
                next = self
                    .placements
                    .iter()
                    .find(|p| (p.image_id, p.id) == key)
                    .and_then(|p| p.parent);
            }
        }
        Ok(())
    }

    fn anchor(
        &self,
        p: &Placement,
        history: usize,
        placeholders: &[Placeholder],
    ) -> Option<(i64, i64)> {
        let mut current = p;
        let mut x = 0i64;
        let mut y = 0i64;
        for _ in 0..=MAX_PARENT_DEPTH {
            if let Some(key) = current.parent {
                x += i64::from(current.horizontal);
                y += i64::from(current.vertical);
                current = self.placements.iter().find(|p| (p.image_id, p.id) == key)?;
            } else if current.virtual_placement {
                let mut min_x = None;
                let mut min_y = None;
                for cell in placeholders.iter().filter(|cell| {
                    cell.image_id == current.image_id
                        && (cell.placement_id == 0 || cell.placement_id == current.id)
                }) {
                    min_x = Some(min_x.map_or(cell.col, |n: i32| n.min(cell.col)));
                    min_y = Some(min_y.map_or(cell.row, |n: i32| n.min(cell.row)));
                }
                return Some((x + i64::from(min_x?), y + i64::from(min_y?)));
            } else {
                return Some((
                    x + i64::from(current.col),
                    y + i64::from(current.row) + history as i64,
                ));
            }
        }
        None
    }

    pub(crate) fn placement_rect(
        &self,
        p: &Placement,
        history: usize,
        placeholders: &[Placeholder],
    ) -> Option<PixelRect> {
        let (x, y) = self.anchor(p, history, placeholders)?;
        let (width, height, inset_x, inset_y) = p.geometry(self.cell_width, self.cell_height);
        Some(PixelRect {
            x: clamp_i32(
                x * i64::from(self.cell_width) + i64::from(p.x_offset) + i64::from(inset_x),
            ),
            y: clamp_i32(
                y * i64::from(self.cell_height) + i64::from(p.y_offset) + i64::from(inset_y),
            ),
            width,
            height,
        })
    }

    /// Resolve placeholders and relative children into the same pixel draws as
    /// ordinary placements. Adjacent tiles with an identical transform merge
    /// into one clip. Only this text-backed path allocates a bounded draw list.
    pub fn draws_with_placeholders(
        &self,
        history: usize,
        placeholders: &[Placeholder],
    ) -> Vec<Draw<'_>> {
        let history = history.min(i32::MAX as usize);
        let mut draws = Vec::new();
        for p in &self.placements {
            if p.virtual_placement {
                continue;
            }
            let Some(image) = self.image(p.image_id) else {
                continue;
            };
            let Some(dest) = self.placement_rect(p, history, placeholders) else {
                continue;
            };
            if i64::from(dest.y) + i64::from(dest.height) <= 0
                || i64::from(dest.y) >= self.visible_rows as i64 * i64::from(self.cell_height)
            {
                continue;
            }
            draws.push(Draw {
                image,
                image_id: p.image_id,
                placement_id: p.id,
                source: p.source,
                dest,
                clip: None,
                z: p.z,
            });
        }
        let mut runs = 0usize;
        for cell in placeholders {
            let Some(p) = self.placements.iter().find(|p| {
                p.virtual_placement
                    && p.image_id == cell.image_id
                    && (cell.placement_id == 0 || p.id == cell.placement_id)
            }) else {
                continue;
            };
            let (width, height, inset_x, inset_y) = p.geometry(self.cell_width, self.cell_height);
            let cols = if p.columns == 0 {
                width.div_ceil(self.cell_width)
            } else {
                p.columns
            };
            let rows = if p.rows == 0 {
                height.div_ceil(self.cell_height)
            } else {
                p.rows
            };
            if cell.image_col >= cols || cell.image_row >= rows {
                continue;
            }
            let dest = PixelRect {
                x: clamp_i32(
                    (i64::from(cell.col) - i64::from(cell.image_col)) * i64::from(self.cell_width)
                        + i64::from(inset_x)
                        + i64::from(p.x_offset),
                ),
                y: clamp_i32(
                    (i64::from(cell.row) - i64::from(cell.image_row)) * i64::from(self.cell_height)
                        + i64::from(inset_y)
                        + i64::from(p.y_offset),
                ),
                width,
                height,
            };
            let clip = PixelRect {
                x: clamp_i32(i64::from(cell.col) * i64::from(self.cell_width)),
                y: clamp_i32(i64::from(cell.row) * i64::from(self.cell_height)),
                width: self
                    .cell_width
                    .saturating_mul(cell.columns.min(cols - cell.image_col)),
                height: self.cell_height,
            };
            if let Some(last) = draws.last_mut()
                && last.image_id == p.image_id
                && last.placement_id == p.id
                && last.dest == dest
                && let Some(previous) = &mut last.clip
                && previous.y == clip.y
                && previous.height == clip.height
                && i64::from(previous.x) + i64::from(previous.width) == i64::from(clip.x)
            {
                previous.width = previous.width.saturating_add(clip.width);
                continue;
            }
            if runs == MAX_PLACEHOLDER_RUNS {
                break;
            }
            let Some(image) = self.image(p.image_id) else {
                continue;
            };
            draws.push(Draw {
                image,
                image_id: p.image_id,
                placement_id: p.id,
                source: p.source,
                dest,
                clip: Some(clip),
                z: p.z,
            });
            runs += 1;
        }
        draws.sort_by_key(|draw| (draw.z, draw.image_id));
        draws
    }

    /// A removed parent owns its relative descendants' lifetime. This is also
    /// run after scrollback trimming and image-ID reuse, not just explicit d=.
    pub(crate) fn prune_dependencies(&mut self) {
        while let Some(index) = self.placements.iter().position(|p| {
            p.parent.is_some_and(|key| {
                !self
                    .placements
                    .iter()
                    .any(|parent| (parent.image_id, parent.id) == key)
            })
        }) {
            let id = self.placements.remove(index).image_id;
            if !self.placements.iter().any(|p| p.image_id == id)
                && let Some(index) = self.images.iter().position(|image| image.id == id)
            {
                self.used -= self.images.remove(index).bytes();
            }
        }
    }
}
