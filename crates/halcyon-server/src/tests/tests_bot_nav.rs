use super::search::{Search, SearchStatus};
use super::*;

use crate::bot::test_map as map;
use map::{MapView, id, stone};

fn nav() -> NavService {
    NavService::new(Budget::Unlimited)
}

fn edge(nav: &mut NavService, view: &impl BlockView, from: Cell, to: Cell) -> Option<Edge> {
    nav.graph.edges(view, from).and_then(|edges| edges.iter().find(|e| e.to == to).copied())
}

/// Two one-block pillars with tops at y = 10, `gap` air blocks apart on x.
fn gap_course(gap: i32) -> MapView {
    let mut view = MapView::default();
    view.pillar(0, 10, 0).pillar(gap + 1, 10, 0);
    view
}

fn ladder() -> halcyon_world::BlockStateId {
    id("minecraft:ladder[facing=north,waterlogged=false]")
}

#[test]
fn stands_classify_floors_slabs_ladders_and_hazards() {
    let mut view = MapView::default();
    view.pillar(0, 10, 0);
    view.pillar(2, 10, 0)
        .set([2, 11, 0], id("minecraft:stone_slab[type=bottom,waterlogged=false]"));
    view.pillar(4, 10, 0).set([4, 12, 0], stone());
    view.set([6, 11, 1], stone()).set([6, 11, 0], ladder());
    view.pillar(8, 10, 0).set([8, 11, 0], id("minecraft:lava[level=0]"));
    let mut nav = nav();
    let at = |nav: &mut NavService, cell| nav.stand(&view, cell);
    assert_eq!(at(&mut nav, [0, 11, 0]).map(|s| s.feet_y), Some(11.0));
    assert_eq!(at(&mut nav, [0, 10, 0]), None, "inside the block");
    assert_eq!(at(&mut nav, [0, 12, 0]), None, "in the air");
    assert_eq!(at(&mut nav, [2, 11, 0]).map(|s| s.feet_y), Some(11.5));
    assert_eq!(at(&mut nav, [4, 11, 0]), None, "one-block ceiling");
    assert!(matches!(at(&mut nav, [6, 11, 0]).map(|s| s.kind), Some(StandKind::Ladder(0))));
    assert_eq!(at(&mut nav, [8, 11, 0]), None, "lava");
}

#[test]
fn gap_jumps_match_vanilla_reach_from_a_single_block() {
    for gap in 1..=3 {
        let view = gap_course(gap);
        let found = edge(&mut nav(), &view, [0, 11, 0], [gap + 1, 11, 0])
            .unwrap_or_else(|| panic!("a {gap}-block gap is jumpable"));
        assert_eq!(found.kind, EdgeKind::Jump);
        assert!(found.robustness > 0.0 && found.robustness <= 1.0);
    }
    for gap in 4..=5 {
        let view = gap_course(gap);
        assert_eq!(edge(&mut nav(), &view, [0, 11, 0], [gap + 1, 11, 0]), None);
    }
}

#[test]
fn jumps_reach_one_block_up_but_not_two() {
    let mut view = MapView::default();
    view.pillar(0, 10, 0).pillar(1, 11, 0).pillar(0, 12, 1).pillar(0, 11, -3);
    let mut nav = nav();
    assert!(
        edge(&mut nav, &view, [0, 11, 0], [1, 12, 0]).is_some_and(|e| e.kind == EdgeKind::Jump)
    );
    assert!(edge(&mut nav, &view, [0, 11, 0], [0, 12, -3]).is_some(), "up one across a gap");
    assert_eq!(edge(&mut nav, &view, [0, 11, 0], [0, 13, 1]), None);
}

#[test]
fn walks_drops_slabs_and_ladders_are_validated() {
    let slab = id("minecraft:stone_slab[type=bottom,waterlogged=false]");
    let mut view = MapView::default();
    view.pillar(0, 10, 0).pillar(1, 10, 0).set([1, 11, 0], slab);
    view.pillar(0, 7, 1).pillar(0, 6, -1);
    let mut nav = NavService::new(Budget::Unlimited);
    assert!(
        edge(&mut nav, &view, [0, 11, 0], [1, 11, 0]).is_some_and(|e| e.kind == EdgeKind::Walk)
    );
    assert!(edge(&mut nav, &view, [0, 11, 0], [0, 8, 1]).is_some_and(|e| e.kind == EdgeKind::Drop));
    // Four blocks down would hurt, and health does not regenerate.
    assert_eq!(edge(&mut nav, &view, [0, 11, 0], [0, 7, -1]), None);

    let mut view = MapView::default();
    view.floor([-2, 2], 10, [-2, 2]);
    for y in 11..15 {
        view.set([0, y, 1], stone()).set([0, y, 0], ladder());
    }
    view.set([0, 14, 1], stone());
    let mut nav = NavService::new(Budget::Unlimited);
    assert!(
        edge(&mut nav, &view, [0, 11, 0], [0, 12, 0]).is_some_and(|e| e.kind == EdgeKind::Climb)
    );
    assert!(edge(&mut nav, &view, [0, 14, 0], [0, 15, 1]).is_some(), "ladder exit onto the wall");
}

#[test]
fn a_low_ceiling_shortens_jumps() {
    let mut view = gap_course(3);
    let mut nav = nav();
    assert!(edge(&mut nav, &view, [0, 11, 0], [4, 11, 0]).is_some());
    view.set([0, 13, 0], stone()).set([1, 13, 0], stone());
    let mut nav = NavService::new(Budget::Unlimited);
    assert_eq!(edge(&mut nav, &view, [0, 11, 0], [4, 11, 0]), None);
}

#[test]
fn continuous_floor_is_walked_not_jumped() {
    let mut view = MapView::default();
    view.floor([-3, 3], 10, [-3, 3]);
    let edges = nav().graph.edges(&view, [0, 11, 0]).unwrap();
    assert_eq!(edges.len(), 8);
    assert!(edges.iter().all(|e| e.kind == EdgeKind::Walk));
}

fn search(
    nav: &mut NavService,
    view: &impl BlockView,
    start: Cell,
    goal: Cell,
    cost: &impl Fn(Cell, &Edge) -> f32,
) -> SearchStatus {
    let mut search = Search::default();
    search.start(start, goal);
    loop {
        match search.step(nav, view, 10_000, cost) {
            SearchStatus::Running => {}
            done => return done,
        }
    }
}

#[test]
fn risk_weight_trades_a_hard_jump_for_a_safe_detour() {
    // A straight three-block gap, or a long walkway around it.
    let mut view = MapView::default();
    view.pillar(0, 10, 0).pillar(4, 10, 0);
    for x in 0..=4 {
        view.pillar(x, 10, 4);
    }
    for z in 1..4 {
        view.pillar(0, 10, z).pillar(4, 10, z);
    }
    let mut nav = nav();
    let route = |status: SearchStatus| match status {
        SearchStatus::Found(path) => path,
        other => panic!("expected a route, got {other:?}"),
    };
    let bold = route(search(&mut nav, &view, [0, 11, 0], [4, 11, 0], &|_, _| 0.0));
    assert_eq!(bold.len(), 1);
    assert_eq!(bold[0].edge.kind, EdgeKind::Jump);
    let careful = route(search(&mut nav, &view, [0, 11, 0], [4, 11, 0], &|_, e: &Edge| {
        if e.kind == EdgeKind::Jump { 1_000.0 } else { 0.0 }
    }));
    assert!(careful.len() > 4);
    assert!(careful.iter().all(|step| step.edge.kind == EdgeKind::Walk));
}

#[test]
fn unreachable_goals_yield_the_closest_partial_route_or_nothing() {
    let mut view = MapView::default();
    view.floor([0, 3], 10, [0, 0]).pillar(12, 10, 0);
    let mut nav = nav();
    match search(&mut nav, &view, [0, 11, 0], [12, 11, 0], &|_, _| 0.0) {
        SearchStatus::Partial(path) => assert_eq!(path.last().unwrap().edge.to, [3, 11, 0]),
        other => panic!("expected a partial route, got {other:?}"),
    }
    match search(&mut nav, &view, [12, 11, 0], [0, 11, 0], &|_, _| 0.0) {
        SearchStatus::Unreachable => {}
        other => panic!("expected no route, got {other:?}"),
    }
}

#[test]
fn spent_budget_pauses_searches_without_losing_progress() {
    let view = gap_course(2);
    let mut nav = NavService::new(Budget::PerWindow {
        limit: std::time::Duration::ZERO,
        window: std::time::Duration::from_secs(3_600),
    });
    let mut search = Search::default();
    search.start([0, 11, 0], [3, 11, 0]);
    assert!(matches!(search.step(&mut nav, &view, 100, &|_, _| 0.0), SearchStatus::Running));
    nav.budget = Budget::Unlimited;
    assert!(matches!(search.step(&mut nav, &view, 100, &|_, _| 0.0), SearchStatus::Found(_)));
}

#[test]
fn arena_edits_invalidate_cached_routes() {
    let world = crate::arena::test_world();
    let mut arena = Arena::default();
    // A two-high wall across the chunk with a doorway at z = 8.
    for z in 0..16 {
        if z != 8 {
            for y in 64..66 {
                arena.edit(&world, [8, y, z], stone()).unwrap();
            }
        }
    }
    let mut nav = nav();
    nav.sync(&arena);
    let found = |nav: &mut NavService, arena: &Arena| {
        matches!(
            search(nav, &arena.view(&world), [4, 64, 8], [12, 64, 8], &|_, _| 0.0),
            SearchStatus::Found(_)
        )
    };
    assert!(found(&mut nav, &arena));
    let cached = nav.graph.cached_cells();
    for y in 64..66 {
        arena.edit(&world, [8, y, 8], stone()).unwrap();
    }
    nav.sync(&arena);
    assert!(nav.graph.cached_cells() < cached);
    assert!(!found(&mut nav, &arena));
}

#[test]
fn the_goal_is_the_highest_standable_cell_nearby() {
    let mut view = MapView::default();
    view.floor([-4, 4], 10, [-4, 4]).pillar(2, 20, 2).pillar(-3, 30, 0);
    view.set([-3, 31, 0], id("minecraft:lava[level=0]"));
    let mut nav = nav();
    assert_eq!(nav.detect_goal(&view, [0, 11, 0], 8), Some([2, 21, 2]));
}
