//! Same-colour neighbour offsets of a CFA position as flat offsets into the mosaic, shared by the two fast loops that
//! walk every photosite of a photo ([`crate::defects::detect`] and the hot-pixel pass): the bound on how many neighbours
//! a position keeps, the flat table, and the test that a whole row of reads stays inside the buffer.

/// The most same-colour neighbours a CFA position keeps (the nearest ones). Both neighbour finders truncate to this, and
/// [`FlatOffsets`] is sized by it, so raising it raises all of them together.
pub(crate) const MAX_NEIGHBOURS: usize = 8;

/// The fewest same-colour neighbours the fast loops accept for a position: they read the 4 nearest unconditionally.
pub(crate) const MIN_NEIGHBOURS: usize = 4;

/// A CFA position's same-colour neighbours as flat offsets into the mosaic (`dy * width + dx`), nearest first.
#[derive(Clone, Copy)]
pub(crate) struct FlatOffsets {
    /// How many of `off` are neighbours (at most [`MAX_NEIGHBOURS`]).
    pub(crate) n: usize,
    pub(crate) off: [isize; MAX_NEIGHBOURS],
}

impl FlatOffsets {
    /// The first [`MAX_NEIGHBOURS`] of `offsets` (as `(dx, dy)`) for a mosaic `w` photosites wide.
    pub(crate) fn new(offsets: &[(isize, isize)], w: usize) -> FlatOffsets {
        let mut off = [0isize; MAX_NEIGHBOURS];
        let mut n = 0;
        for (slot, &(dx, dy)) in off.iter_mut().zip(offsets) {
            *slot = dy.saturating_mul(w as isize).saturating_add(dx);
            n += 1;
        }
        FlatOffsets { n, off }
    }

    /// The neighbours' offsets.
    #[inline]
    pub(crate) fn offsets(&self) -> &[isize] {
        self.off.get(..self.n).unwrap_or(&[])
    }

    /// Does every neighbour read of the photosites `start, start + step, ..` of row `row` (up to column `x1`, exclusive)
    /// land inside a buffer of `len` samples? The lowest and highest offsets are seeded with 0, so the test holds
    /// whatever their signs: the photosite itself is read too. When it is true the loop may index without checking.
    #[inline]
    pub(crate) fn row_inside(&self, row: usize, start: usize, x1: usize, len: usize) -> bool {
        let (lo, hi) = self.offsets().iter().fold((0isize, 0isize), |(l, h), &o| (l.min(o), h.max(o)));
        (row + start) as isize + lo >= 0 && ((row + x1.max(1) - 1) as isize).saturating_add(hi) < len as isize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_keeps_the_first_max_neighbours_and_saturates() {
        let offs: Vec<(isize, isize)> = (0..MAX_NEIGHBOURS as isize + 3).map(|i| (i, i - 2)).collect();
        let f = FlatOffsets::new(&offs, 100);
        assert_eq!(f.n, MAX_NEIGHBOURS);
        assert_eq!(f.offsets().len(), MAX_NEIGHBOURS);
        assert_eq!(f.off[0], -200);
        assert_eq!(f.off[2], 2);
        assert_eq!(FlatOffsets::new(&[], 100).offsets(), &[] as &[isize]);
        assert_eq!(FlatOffsets::new(&[(1, isize::MAX)], 100).off[0], isize::MAX);
    }

    /// At, below and above each edge of the buffer; offsets of one sign only (the 0 seed: the photosite itself is read).
    #[test]
    fn a_row_is_inside_only_while_every_read_is() {
        let f = FlatOffsets::new(&[(-2, 0), (2, 0), (0, -1), (0, 1)], 10); // offsets -2, 2, -10, 10
        // a row of 10 in a buffer of 30: row 10 (the middle one), columns 2..8
        assert!(f.row_inside(10, 2, 8, 30));
        assert!(!f.row_inside(0, 2, 8, 30), "reads 10 before the buffer");
        assert!(!f.row_inside(20, 2, 8, 30), "reads past the end");
        // the whole middle row: its first read is index 0 and its last is 29, the buffer's last sample
        assert!(f.row_inside(10, 0, 10, 30));
        assert!(!f.row_inside(10, 0, 10, 29), "one sample shorter");
        assert!(!f.row_inside(9, 0, 10, 30), "one row up reads index -1");
        let after = FlatOffsets::new(&[(1, 0), (2, 0), (3, 0), (4, 0)], 10);
        // positive offsets only: the lowest read is the photosite itself
        assert!(after.row_inside(0, 0, 5, 9));
        assert!(!after.row_inside(0, 0, 5, 8), "column 4 + 4 = 8 is the 9th sample");
        let before = FlatOffsets::new(&[(-1, 0), (-2, 0), (-3, 0), (-4, 0)], 10);
        assert!(before.row_inside(0, 4, 8, 8));
        assert!(!before.row_inside(0, 4, 8, 7), "negative offsets only: the photosite itself, column 7, is the highest read");
        assert!(!before.row_inside(0, 3, 8, 8), "column 3 - 4 is before the buffer");
        // no neighbours, an empty row, no buffer
        let none = FlatOffsets::new(&[], 10);
        assert!(none.row_inside(0, 0, 1, 1));
        assert!(!none.row_inside(0, 0, 1, 0));
        assert!(!f.row_inside(usize::MAX / 4, 0, 1, 10));
    }
}
