//! Hit targets recorded from the rendered frame, including list offsets and modal boundaries.

#[cfg(test)]
mod tests;

use super::activity::BlockRef;
use ratatui::layout::{Position, Rect};

#[derive(Clone, Debug)]
pub(crate) enum MouseTarget {
    Messages,
    Activity(BlockRef),
    Input,
    Sessions,
    Session(String),
    Settings,
    ActivityList,
    ActivityItem(BlockRef),
    ActivityDetail,
    ActivityTab(usize),
    ActivityBack,
    ActivityClose,
}

#[derive(Debug, Default)]
pub(crate) struct MouseMap {
    regions: Vec<(Rect, MouseTarget)>,
    pub dragging_input: bool,
}

impl MouseMap {
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn block_background(&mut self) {
        self.clear();
        self.dragging_input = false;
    }

    pub fn register(&mut self, area: Rect, target: MouseTarget) {
        if !area.is_empty() {
            self.regions.push((area, target));
        }
    }

    pub fn hit(&self, position: Position) -> Option<(Rect, MouseTarget)> {
        self.regions
            .iter()
            .rev()
            .find(|(area, _)| area.contains(position))
            .cloned()
    }

    pub fn input_area(&self) -> Option<Rect> {
        self.regions
            .iter()
            .find_map(|(area, target)| matches!(target, MouseTarget::Input).then_some(*area))
    }
}
