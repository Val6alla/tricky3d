//! Bounding volume hierarchy over axis-aligned boxes (usually one per triangle): every pair of
//! overlapping boxes, the boxes a query box or a segment touches. The candidate search behind
//! self-intersection tests, Wrap part surface and ray casting.
//!
//! A binary tree in flat arrays, built by median splits on the longest axis of the box centres,
//! with at most four items per leaf. The build partitions the item list in place, so every subtree
//! owns a contiguous run of it and the leaves keep their items' boxes next to each other.

use std::cmp::Reverse;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::exact::{P3, sub, vertex};
use crate::mesh::Mesh;

/// [min, max] corners.
pub type Aabb = [[f64; 3]; 2];

/// A box that holds nothing: it overlaps nothing and `grow`/`include` replace it.
pub const EMPTY: Aabb = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];

/// Most items in one leaf.
const LEAF: usize = 4;

/// Below this many items the parallel pair search and build stay on the calling thread.
const PARALLEL_MIN: usize = 2048;

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    /// The node's items are `items[start..end]`.
    start: u32,
    end: u32,
    /// The first child (the second follows it), or 0 for a leaf: the root is node 0, nobody's child.
    left: u32,
}

impl Node {
    fn is_leaf(&self) -> bool {
        self.left == 0
    }

    fn count(&self) -> u32 {
        self.end - self.start
    }
}

pub struct Bvh {
    nodes: Vec<Node>,
    /// Item ids in leaf order.
    items: Vec<u32>,
    /// The items' boxes in the same order.
    boxes: Vec<Aabb>,
}

/// A piece of the pair search: the pairs within one subtree, or between two subtrees whose boxes
/// overlap. Pieces never share a pair, so threads can take them independently.
#[derive(Clone, Copy, Debug)]
enum Task {
    Within(u32),
    Between(u32, u32),
}

/// Splits a run of box centres (with their items) at its middle along the longest axis of the
/// centres, then each half the same way, until a run fits in a leaf: the order `Bvh::new` lays its
/// leaves out in. Each half depends only on its own run, so large halves are split on separate
/// threads and the order comes out the same.
fn partition(work: &mut [(P3, u32)]) {
    if work.len() <= LEAF {
        return;
    }
    let mut spread = EMPTY;
    for &(c, _) in work.iter() {
        include(&mut spread, c);
    }
    let axis = longest_axis(&spread);
    let mid = work.len() / 2;
    work.select_nth_unstable_by(mid, |x, y| x.0[axis].total_cmp(&y.0[axis]));
    let large = work.len() >= PARALLEL_MIN;
    let (left, right) = work.split_at_mut(mid);
    if large {
        rayon::join(|| partition(left), || partition(right));
    } else {
        partition(left);
        partition(right);
    }
}

impl Bvh {
    /// A hierarchy over `boxes`; items are their indices.
    pub fn new(boxes: &[Aabb]) -> Bvh {
        assert!(
            boxes.len() < u32::MAX as usize,
            "too many boxes for one Bvh"
        );
        let n = boxes.len();
        // Box centres with their items, partitioned in place: each split reads its own run in
        // order instead of jumping around the boxes.
        let mut work: Vec<(P3, u32)> = boxes
            .iter()
            .enumerate()
            .map(|(i, b)| (center(b), i as u32))
            .collect();
        partition(&mut work);
        // The tree's shape depends only on n: every node splits its run at the middle.
        let mut nodes = Vec::new();
        if n > 0 {
            // Every split leaves at least two items on each side, so leaves hold two or more items
            // (unless n < 2) and there are fewer than n nodes.
            nodes.reserve(n);
            nodes.push(Node {
                bounds: EMPTY,
                start: 0,
                end: n as u32,
                left: 0,
            });
            let mut stack = vec![0usize];
            while let Some(i) = stack.pop() {
                let (start, end) = (nodes[i].start as usize, nodes[i].end as usize);
                if end - start <= LEAF {
                    continue;
                }
                let mid = start + (end - start) / 2;
                let left = nodes.len();
                nodes[i].left = left as u32;
                for (s, e) in [(start, mid), (mid, end)] {
                    nodes.push(Node {
                        bounds: EMPTY,
                        start: s as u32,
                        end: e as u32,
                        left: 0,
                    });
                }
                stack.push(left + 1);
                stack.push(left);
            }
        }
        let items: Vec<u32> = work.iter().map(|&(_, i)| i).collect();
        let boxes: Vec<Aabb> = items.iter().map(|&i| boxes[i as usize]).collect();
        // Bounds from the leaves up: children come after their parent, so a backward pass meets
        // them first.
        for i in (0..nodes.len()).rev() {
            let node = nodes[i];
            let mut bounds = EMPTY;
            if node.is_leaf() {
                for b in &boxes[node.start as usize..node.end as usize] {
                    grow(&mut bounds, b);
                }
            } else {
                grow(&mut bounds, &nodes[node.left as usize].bounds);
                grow(&mut bounds, &nodes[node.left as usize + 1].bounds);
            }
            nodes[i].bounds = bounds;
        }
        Bvh {
            nodes,
            items,
            boxes,
        }
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The box around every item; None when there are none.
    pub fn bounds(&self) -> Option<Aabb> {
        self.nodes.first().map(|root| root.bounds)
    }

    /// Calls `f(i, j)` once for every pair of items i < j whose boxes overlap (touching counts).
    pub fn self_pairs(&self, mut f: impl FnMut(u32, u32)) {
        if !self.nodes.is_empty() {
            self.run(Task::Within(0), &mut f);
        }
    }

    /// The overlapping pairs (i < j) for which `keep(i, j)` is true, found on several threads.
    /// Sorted. For tests that are expensive per pair, such as exact triangle intersection.
    pub fn self_pairs_parallel(&self, keep: impl Fn(u32, u32) -> bool + Sync) -> Vec<(u32, u32)> {
        // As many threads as the worker pool, which leaves a core free in the app (src/workers.rs).
        // The calling thread only waits for them.
        let threads = rayon::current_num_threads();
        let mut pairs = Vec::new();
        if threads < 2 || self.len() < PARALLEL_MIN {
            self.self_pairs(|i, j| {
                if keep(i, j) {
                    pairs.push((i, j));
                }
            });
        } else {
            // Many more tasks than threads, biggest first, taken in turn: the threads finish at
            // about the same time even though the tasks differ a lot in size.
            let tasks = self.tasks(threads * 16);
            let next = AtomicUsize::new(0);
            pairs = std::thread::scope(|scope| {
                let workers: Vec<_> = (0..threads.min(tasks.len()))
                    .map(|_| {
                        scope.spawn(|| {
                            crate::workers::lower_priority();
                            let mut found = Vec::new();
                            while let Some(&task) = tasks.get(next.fetch_add(1, Ordering::Relaxed))
                            {
                                self.run(task, &mut |i, j| {
                                    if keep(i, j) {
                                        found.push((i, j));
                                    }
                                });
                            }
                            found
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                    .collect()
            });
        }
        pairs.sort_unstable();
        pairs
    }

    /// Calls `f(i)` for every item whose box overlaps `query` (touching counts).
    pub fn query(&self, query: &Aabb, mut f: impl FnMut(u32)) {
        self.visit(|b| overlap(b, query), &mut f);
    }

    /// Calls `f(i)` for every item whose box the segment from `a` to `b` touches. Conservative: a
    /// box the segment misses by a hair (about 1e-12 of the coordinates' size) may be reported too,
    /// never one it touches left out.
    pub fn segment(&self, a: P3, b: P3, mut f: impl FnMut(u32)) {
        let Some(root) = self.nodes.first() else {
            return;
        };
        let d = sub(b, a);
        let size = [a, b, root.bounds[0], root.bounds[1]]
            .iter()
            .flatten()
            .filter(|x| x.is_finite())
            .fold(0.0f64, |m, x| m.max(x.abs()));
        // The slab test below rounds by a few units in the last place of `size`; boxes grown by
        // this margin therefore still contain every point of the exact segment they touched.
        let margin = size * 1e-12;
        let touches = |bx: &Aabb| {
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for k in 0..3 {
                let (min, max) = (bx[0][k] - margin, bx[1][k] + margin);
                if d[k] == 0.0 {
                    if a[k] < min || a[k] > max {
                        return false;
                    }
                } else {
                    let (near, far) = if d[k] > 0.0 { (min, max) } else { (max, min) };
                    lo = lo.max((near - a[k]) / d[k]);
                    hi = hi.min((far - a[k]) / d[k]);
                    if lo > hi {
                        return false;
                    }
                }
            }
            true
        };
        self.visit(touches, &mut f);
    }

    /// Depth-first over the nodes whose box `enter` accepts, calling `f` for the items whose own
    /// box it accepts.
    fn visit(&self, enter: impl Fn(&Aabb) -> bool, f: &mut impl FnMut(u32)) {
        if self.nodes.is_empty() {
            return;
        }
        // Depth-first with both children pushed: the stack never holds more than the tree's depth
        // plus one, about 32 for four billion items.
        let mut stack = Vec::with_capacity(64);
        stack.push(0u32);
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n as usize];
            if !enter(&node.bounds) {
                continue;
            }
            if node.is_leaf() {
                for i in node.start as usize..node.end as usize {
                    if enter(&self.boxes[i]) {
                        f(self.items[i]);
                    }
                }
            } else {
                stack.push(node.left + 1);
                stack.push(node.left);
            }
        }
    }

    /// Runs one task of the pair search to the end.
    fn run(&self, task: Task, f: &mut impl FnMut(u32, u32)) {
        let mut stack = vec![task];
        while let Some(task) = stack.pop() {
            self.step(task, f, &mut |t| stack.push(t));
        }
    }

    /// One step of the pair search: report the pairs of a task over leaves, or split the task into
    /// smaller ones (only those whose boxes overlap).
    fn step(&self, task: Task, f: &mut impl FnMut(u32, u32), push: &mut impl FnMut(Task)) {
        match task {
            Task::Within(n) => {
                let node = &self.nodes[n as usize];
                if node.is_leaf() {
                    let (start, end) = (node.start as usize, node.end as usize);
                    for i in start..end {
                        for j in i + 1..end {
                            if overlap(&self.boxes[i], &self.boxes[j]) {
                                let (x, y) = ordered(self.items[i], self.items[j]);
                                f(x, y);
                            }
                        }
                    }
                } else {
                    let l = node.left;
                    push(Task::Within(l));
                    push(Task::Within(l + 1));
                    if overlap(
                        &self.nodes[l as usize].bounds,
                        &self.nodes[l as usize + 1].bounds,
                    ) {
                        push(Task::Between(l, l + 1));
                    }
                }
            }
            Task::Between(a, b) => {
                let (na, nb) = (&self.nodes[a as usize], &self.nodes[b as usize]);
                if na.is_leaf() && nb.is_leaf() {
                    for i in na.start as usize..na.end as usize {
                        for j in nb.start as usize..nb.end as usize {
                            if overlap(&self.boxes[i], &self.boxes[j]) {
                                let (x, y) = ordered(self.items[i], self.items[j]);
                                f(x, y);
                            }
                        }
                    }
                } else if nb.is_leaf() || (!na.is_leaf() && na.count() >= nb.count()) {
                    // Descend into the bigger side.
                    for c in [na.left, na.left + 1] {
                        if overlap(&self.nodes[c as usize].bounds, &nb.bounds) {
                            push(Task::Between(c, b));
                        }
                    }
                } else {
                    for c in [nb.left, nb.left + 1] {
                        if overlap(&na.bounds, &self.nodes[c as usize].bounds) {
                            push(Task::Between(a, c));
                        }
                    }
                }
            }
        }
    }

    /// The pair search cut into at least `want` tasks where the tree allows, biggest first.
    fn tasks(&self, want: usize) -> Vec<Task> {
        let mut tasks = vec![Task::Within(0)];
        let mut none = |_: u32, _: u32| unreachable!("tasks over leaves are never split");
        loop {
            let mut next = Vec::with_capacity(tasks.len() * 3);
            let mut split = false;
            for &task in &tasks {
                if self.over_leaves(task) {
                    next.push(task);
                } else {
                    self.step(task, &mut none, &mut |t| next.push(t));
                    split = true;
                }
            }
            tasks = next;
            if !split || tasks.len() >= want {
                break;
            }
        }
        // Stable, so the order (and with it which thread takes what) is the same every run.
        tasks.sort_by_key(|&t| {
            Reverse(match t {
                Task::Within(n) => self.nodes[n as usize].count(),
                Task::Between(a, b) => {
                    self.nodes[a as usize].count() + self.nodes[b as usize].count()
                }
            })
        });
        tasks
    }

    fn over_leaves(&self, task: Task) -> bool {
        match task {
            Task::Within(n) => self.nodes[n as usize].is_leaf(),
            Task::Between(a, b) => {
                self.nodes[a as usize].is_leaf() && self.nodes[b as usize].is_leaf()
            }
        }
    }
}

/// One box per triangle of the mesh.
pub fn triangle_boxes(mesh: &Mesh) -> Vec<Aabb> {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let mut b = EMPTY;
            for &v in t {
                include(&mut b, vertex(mesh, v));
            }
            b
        })
        .collect()
}

/// One box per triangle of `triangles` over `points` (for meshes held as f64 points).
pub fn boxes_of(points: &[P3], triangles: &[[u32; 3]]) -> Vec<Aabb> {
    triangles
        .iter()
        .map(|t| {
            let mut b = EMPTY;
            for &v in t {
                include(&mut b, points[v as usize]);
            }
            b
        })
        .collect()
}

/// True when the boxes overlap or touch.
#[inline]
pub fn overlap(a: &Aabb, b: &Aabb) -> bool {
    a[0][0] <= b[1][0]
        && b[0][0] <= a[1][0]
        && a[0][1] <= b[1][1]
        && b[0][1] <= a[1][1]
        && a[0][2] <= b[1][2]
        && b[0][2] <= a[1][2]
}

/// Grows `b` to hold `other`.
#[inline]
pub fn grow(b: &mut Aabb, other: &Aabb) {
    for k in 0..3 {
        b[0][k] = b[0][k].min(other[0][k]);
        b[1][k] = b[1][k].max(other[1][k]);
    }
}

/// Grows `b` to hold `p`.
#[inline]
pub fn include(b: &mut Aabb, p: P3) {
    for (k, &x) in p.iter().enumerate() {
        b[0][k] = b[0][k].min(x);
        b[1][k] = b[1][k].max(x);
    }
}

fn center(b: &Aabb) -> P3 {
    [0, 1, 2].map(|k| (b[0][k] + b[1][k]) * 0.5)
}

fn longest_axis(b: &Aabb) -> usize {
    let e = [0, 1, 2].map(|k| b[1][k] - b[0][k]);
    if e[0] >= e[1] && e[0] >= e[2] {
        0
    } else if e[1] >= e[2] {
        1
    } else {
        2
    }
}

#[inline]
fn ordered(i: u32, j: u32) -> (u32, u32) {
    if i < j { (i, j) } else { (j, i) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;
    use crate::repair::exact::testing::{Rng, torus};
    use num_rational::Ratio;
    use std::time::Instant;

    /// Boxes with integer corners on a small grid, so many touch exactly and some are points.
    fn grid_boxes(rng: &mut Rng, n: usize, grid: i64, size: i64) -> Vec<[[i64; 3]; 2]> {
        (0..n)
            .map(|_| {
                let lo = [0, 1, 2].map(|_| rng.int(0, grid));
                let hi = lo.map(|x| x + rng.int(0, size));
                [lo, hi]
            })
            .collect()
    }

    fn to_f64(b: &[[i64; 3]; 2]) -> Aabb {
        [b[0].map(|x| x as f64), b[1].map(|x| x as f64)]
    }

    fn brute_pairs(boxes: &[Aabb]) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                if overlap(&boxes[i], &boxes[j]) {
                    out.push((i as u32, j as u32));
                }
            }
        }
        out
    }

    fn collected_pairs(bvh: &Bvh) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        bvh.self_pairs(|i, j| {
            assert!(i < j);
            out.push((i, j));
        });
        out.sort_unstable();
        out
    }

    /// The tree the build made before it split runs on several threads: every split one after
    /// the other, on one thread.
    fn sequential_order(boxes: &[Aabb]) -> Vec<u32> {
        let mut work: Vec<(P3, u32)> = boxes
            .iter()
            .enumerate()
            .map(|(i, b)| (center(b), i as u32))
            .collect();
        let mut stack = vec![(0usize, work.len())];
        while let Some((start, end)) = stack.pop() {
            if end - start <= LEAF {
                continue;
            }
            let mut spread = EMPTY;
            for &(c, _) in &work[start..end] {
                include(&mut spread, c);
            }
            let axis = longest_axis(&spread);
            let mid = start + (end - start) / 2;
            work[start..end]
                .select_nth_unstable_by(mid - start, |x, y| x.0[axis].total_cmp(&y.0[axis]));
            stack.push((mid, end));
            stack.push((start, mid));
        }
        work.iter().map(|&(_, i)| i).collect()
    }

    #[test]
    fn the_parallel_build_lays_out_the_same_tree() {
        let mut rng = Rng(11);
        for &n in &[0usize, 3, 5, 2047, 2048, 2049, 50_000, 200_001] {
            let boxes: Vec<Aabb> = grid_boxes(&mut rng, n, 1000, 30)
                .iter()
                .map(to_f64)
                .collect();
            assert_eq!(Bvh::new(&boxes).items, sequential_order(&boxes), "n = {n}");
        }
    }

    #[test]
    fn pairs_match_brute_force() {
        let mut rng = Rng(1);
        for &n in &[0usize, 1, 2, 3, 4, 5, 9, 33, 257, 3000] {
            for &(grid, size) in &[(20, 4), (6, 2), (100, 1), (3, 3)] {
                let boxes: Vec<Aabb> = grid_boxes(&mut rng, n, grid, size)
                    .iter()
                    .map(to_f64)
                    .collect();
                let bvh = Bvh::new(&boxes);
                assert_eq!(bvh.len(), n);
                let expected = brute_pairs(&boxes);
                let found = collected_pairs(&bvh);
                // Sorted equality also proves no pair came twice.
                assert_eq!(found, expected, "n {n} grid {grid} size {size}");
            }
        }
    }

    #[test]
    fn identical_and_point_boxes() {
        let same = vec![[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]; 50];
        assert_eq!(collected_pairs(&Bvh::new(&same)).len(), 50 * 49 / 2);
        let points = vec![[[7.0; 3], [7.0; 3]]; 20];
        assert_eq!(collected_pairs(&Bvh::new(&points)).len(), 20 * 19 / 2);
        // Points along a line, one unit apart: none overlap, neighbours of unit boxes touch.
        let line: Vec<Aabb> = (0..100).map(|i| [[i as f64, 0.0, 0.0]; 2]).collect();
        assert!(collected_pairs(&Bvh::new(&line)).is_empty());
        let unit: Vec<Aabb> = (0..100)
            .map(|i| [[i as f64, 0.0, 0.0], [i as f64 + 1.0, 1.0, 1.0]])
            .collect();
        assert_eq!(
            collected_pairs(&Bvh::new(&unit)),
            (0..99).map(|i| (i, i + 1)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn parallel_pairs_match_and_are_filtered_and_sorted() {
        let mut rng = Rng(2);
        for &n in &[0usize, 7, 1000, 2047, 2048, 20_000] {
            let boxes: Vec<Aabb> = grid_boxes(&mut rng, n, 60, 3).iter().map(to_f64).collect();
            let bvh = Bvh::new(&boxes);
            let keep = |i: u32, j: u32| !(i * 7 + j * 3).is_multiple_of(5);
            let expected: Vec<(u32, u32)> = brute_pairs(&boxes)
                .into_iter()
                .filter(|&(i, j)| keep(i, j))
                .collect();
            assert_eq!(bvh.self_pairs_parallel(keep), expected, "n {n}");
        }
    }

    #[test]
    fn tasks_cover_the_search_exactly_once() {
        let mut rng = Rng(3);
        let boxes: Vec<Aabb> = grid_boxes(&mut rng, 5000, 40, 3)
            .iter()
            .map(to_f64)
            .collect();
        let bvh = Bvh::new(&boxes);
        for want in [1, 2, 16, 500, 100_000] {
            let mut found = Vec::new();
            for task in bvh.tasks(want) {
                bvh.run(task, &mut |i, j| found.push((i, j)));
            }
            found.sort_unstable();
            assert_eq!(found, brute_pairs(&boxes), "want {want}");
        }
    }

    #[test]
    fn query_matches_brute_force() {
        let mut rng = Rng(4);
        let boxes: Vec<Aabb> = grid_boxes(&mut rng, 2000, 30, 3)
            .iter()
            .map(to_f64)
            .collect();
        let bvh = Bvh::new(&boxes);
        for q in grid_boxes(&mut rng, 300, 30, 6).iter().map(to_f64) {
            let mut found = Vec::new();
            bvh.query(&q, |i| found.push(i));
            found.sort_unstable();
            let expected: Vec<u32> = (0..boxes.len() as u32)
                .filter(|&i| overlap(&boxes[i as usize], &q))
                .collect();
            assert_eq!(found, expected);
        }
        let mut none = 0;
        Bvh::new(&[]).query(&[[0.0; 3], [1.0; 3]], |_| none += 1);
        assert_eq!(none, 0);
    }

    /// Whether the segment touches the box, in exact rational arithmetic.
    fn segment_touches_exactly(a: [i64; 3], b: [i64; 3], bx: &[[i64; 3]; 2]) -> bool {
        let mut lo = Ratio::from_integer(0i64);
        let mut hi = Ratio::from_integer(1i64);
        for k in 0..3 {
            let d = b[k] - a[k];
            if d == 0 {
                if a[k] < bx[0][k] || a[k] > bx[1][k] {
                    return false;
                }
                continue;
            }
            let t0 = Ratio::new(bx[0][k] - a[k], d);
            let t1 = Ratio::new(bx[1][k] - a[k], d);
            let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
            lo = lo.max(t0);
            hi = hi.min(t1);
            if lo > hi {
                return false;
            }
        }
        true
    }

    #[test]
    fn segments_find_every_box_they_touch() {
        let mut rng = Rng(5);
        let int_boxes = grid_boxes(&mut rng, 3000, 30, 3);
        let boxes: Vec<Aabb> = int_boxes.iter().map(to_f64).collect();
        let bvh = Bvh::new(&boxes);
        let mut grazing = 0;
        for round in 0..400 {
            let a = [0, 1, 2].map(|_| rng.int(-5, 38));
            let mut b = [0, 1, 2].map(|_| rng.int(-5, 38));
            // Axis-parallel segments and single points too.
            if round % 5 == 0 {
                b[1] = a[1];
                b[2] = a[2];
            }
            if round % 17 == 0 {
                b = a;
            }
            let (af, bf) = (a.map(|x| x as f64), b.map(|x| x as f64));
            let mut found = vec![false; boxes.len()];
            bvh.segment(af, bf, |i| found[i as usize] = true);
            for (i, bx) in int_boxes.iter().enumerate() {
                let exact = segment_touches_exactly(a, b, bx);
                if exact {
                    assert!(found[i], "missed box {bx:?} for segment {a:?} {b:?}");
                    // Touching only at a box corner, edge or face happens on this grid.
                    let inner = [
                        [0, 1, 2].map(|k| bx[0][k] * 4 + 1),
                        [0, 1, 2].map(|k| bx[1][k] * 4 - 1),
                    ];
                    grazing += !segment_touches_exactly(a.map(|x| x * 4), b.map(|x| x * 4), &inner)
                        as usize;
                } else if found[i] {
                    // Only boxes the segment misses by less than the margin may slip through.
                    let g = 1e-9 * 40.0;
                    let grown = [bx[0].map(|x| x as f64 - g), bx[1].map(|x| x as f64 + g)];
                    let mut near = Vec::new();
                    Bvh::new(&[grown]).segment(af, bf, |j| near.push(j));
                    assert!(
                        !near.is_empty(),
                        "box {bx:?} reported for segment {a:?} {b:?}"
                    );
                }
            }
        }
        assert!(grazing > 50, "{grazing}");
    }

    #[test]
    fn segment_in_any_direction_through_a_tiny_box_far_away() {
        // Coordinates in the millions and a box a millionth wide: the slab test must not round it
        // away, whatever the direction.
        let mut rng = Rng(6);
        for _ in 0..2000 {
            let c = [0, 1, 2].map(|_| 1e6 + rng.unit() * 10.0);
            let bx = [c.map(|x| x - 1e-6), c.map(|x| x + 1e-6)];
            let dir = [0, 1, 2].map(|_| rng.unit() * 2.0 - 1.0);
            let s = 1e3 * rng.unit().max(0.01);
            // The rounding of a and b moves the line by about 1e-10, far less than the box size.
            let a = [0, 1, 2].map(|k| c[k] - dir[k] * 1e3);
            let b = [0, 1, 2].map(|k| c[k] + dir[k] * s);
            let mut hit = false;
            Bvh::new(&[bx]).segment(a, b, |_| hit = true);
            assert!(hit, "{a:?} {b:?} {bx:?}");
        }
    }

    #[test]
    fn boxes_of_triangles() {
        let cube = make_box([0.0; 3], [2.0, 3.0, 4.0]);
        let boxes = triangle_boxes(&cube);
        assert_eq!(boxes.len(), 12);
        // The first triangle is on the bottom face (z = 0).
        assert_eq!(boxes[0], [[0.0, 0.0, 0.0], [2.0, 3.0, 0.0]]);
        let points: Vec<P3> = cube
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(f64::from))
            .collect();
        let triangles: Vec<[u32; 3]> = cube.indices.as_chunks::<3>().0.to_vec();
        assert_eq!(boxes_of(&points, &triangles), boxes);
        assert_eq!(Bvh::new(&boxes).bounds(), Some([[0.0; 3], [2.0, 3.0, 4.0]]));
        assert_eq!(Bvh::new(&[]).bounds(), None);
    }

    /// Build and pair search on a million-triangle torus. Run with
    /// `cargo test --release --lib repair::bvh -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn million_triangle_timing() {
        let mesh = torus(30.0, 10.0, 1000, 500);
        assert_eq!(mesh.triangle_count(), 1_000_000);
        let started = Instant::now();
        let boxes = triangle_boxes(&mesh);
        let boxed = started.elapsed();
        let bvh = Bvh::new(&boxes);
        let built = started.elapsed();
        let mut count = 0u64;
        bvh.self_pairs(|_, _| count += 1);
        let paired = started.elapsed();
        let kept = bvh.self_pairs_parallel(|i, j| (i ^ j) & 1023 == 0).len();
        let parallel = started.elapsed();
        println!(
            "1M triangles: boxes {boxed:?}, build {:?}, self_pairs {count} pairs {:?}, \
             self_pairs_parallel ({kept} kept) {:?}",
            built - boxed,
            paired - built,
            parallel - paired
        );
    }
}
