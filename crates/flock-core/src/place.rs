//! Ghost placement validation (SPEC §7).

use crate::course::ObjKind;
use crate::world::{World, PLACE_LOG_LEN};
use crate::{Accepted, Event, Phase, PlaceRequest, PlaceResult, PlayerId, Reject, Role, Tool};

fn is_ghost(w: &World, player: PlayerId) -> Option<f32> {
    if w.phase() != Phase::Running {
        return None;
    }
    w.player(player).and_then(|p| p.ghost_energy())
}

/// Everything after `NotGhost`/`Duplicate`. Returns the cost.
fn validate(w: &World, player: PlayerId, tool: Tool, lane: u8, row: u32) -> Result<f32, Reject> {
    let t = w.tuning();
    let energy = is_ghost(w, player).ok_or(Reject::NotGhost)?;
    let course = w.course();
    // Band (also: lane range, row generated).
    if lane > 2 || !w.placement_band().contains(row) || course.row(row).is_none() {
        return Err(Reject::OutsideBand);
    }
    let now = w.tick();
    let objs = course.placement_objs(player, course.next_architect_id(), tool, lane, row, now);
    // Reaction window from every living runner at max plausible speed.
    let v = (w.v_course() + t.jump_fwd_boost) as f64;
    let reach = t.min_reaction_s as f64 * v;
    let front = objs.iter().map(|o| o.z0()).fold(f64::INFINITY, f64::min);
    let back = objs.iter().map(|o| o.z1()).fold(f64::NEG_INFINITY, f64::max);
    for r in w.runners() {
        let rz = r.pos.z;
        let rf = rz + t.body_radius as f64;
        if back >= rz - t.body_radius as f64 && front - rf < reach {
            return Err(Reject::TooClose);
        }
    }
    // Occupancy.
    let existing = course.row_objects(row);
    let in_cell = |o: &&crate::Obj| o.covers_lane(lane);
    let occupied = existing.iter().filter(in_cell).any(|o| {
        let pending = o.by.is_some() && !o.is_live(now);
        pending
            || match tool {
                Tool::Hay | Tool::Beam => o.kind.is_solid(),
                Tool::Mud => o.kind == ObjKind::Mud,
                Tool::Coins => o.kind == ObjKind::Coin,
            }
    });
    if occupied {
        return Err(Reject::Occupied);
    }
    if tool.is_solid() {
        // Shared density of architect solids.
        let count = |r: u32| course.placed().iter().filter(|o| o.row == r && o.kind.is_solid()).count();
        if count(row) + 1 > t.density_per_row as usize {
            return Err(Reject::Density);
        }
        for w0 in row.saturating_sub(3)..=row {
            let n: usize = (w0..w0 + 4).map(count).sum();
            if n + 1 > t.density_per_4_rows as usize {
                return Err(Reject::Density);
            }
        }
    }
    if tool.is_solid() || tool == Tool::Mud {
        // Feasibility with this (and all pending) placements. Mud counts:
        // no jump can start in it, so mud in front of a jump-only obstacle
        // can make a lane (or the course) impassable.
        let speed = w.speed_model();
        if course.feasible_window(&|z| speed.at(z), &objs).is_none() {
            return Err(Reject::Unsafe);
        }
    }
    let cost = t.tool_cost(tool);
    if energy + 1e-4 < cost {
        return Err(Reject::NoEnergy);
    }
    Ok(cost)
}

pub(crate) fn check(w: &World, player: PlayerId, tool: Tool, lane: u8, row: u32) -> Result<f32, Reject> {
    validate(w, player, tool, lane, row)
}

pub(crate) fn place(w: &mut World, req: PlaceRequest) -> PlaceResult {
    if is_ghost(w, req.player).is_none() {
        return Err(Reject::NotGhost);
    }
    let slot = req.player.0 as usize;
    if slot >= w.place_log.len() {
        return Err(Reject::NotGhost);
    }
    if let Some((_, res)) = w.place_log[slot].iter().find(|(r, _)| *r == req.req) {
        return *res;
    }
    let log = &w.place_log[slot];
    if log.len() >= PLACE_LOG_LEN && log.iter().all(|(r, _)| req.req < *r) {
        return Err(Reject::Duplicate);
    }
    let res = validate(w, req.player, req.tool, req.lane, req.row).map(|cost| {
        let t = w.tuning().clone();
        let obj = w.course().next_architect_id();
        let solid_at = w.tick() + t.ticks(t.materialise_s);
        w.course_mut().add_placement(req.player, obj, req.tool, req.lane, req.row, solid_at);
        if req.tool.is_solid() || req.tool == Tool::Mud {
            let speed = w.speed_model();
            w.course_mut().recompute_frontier(&|z| speed.at(z));
        }
        if let Some(i) = w.players().iter().position(|p| p.id() == req.player) {
            let p = &mut w.players_mut()[i];
            p.placements += 1;
            if let Role::Ghost { energy, placed } = &mut p.role {
                *energy -= cost;
                *placed += 1;
            }
        }
        w.push_queued(Event::Placed { by: req.player, obj, tool: req.tool, lane: req.lane, row: req.row, solid_at });
        Accepted { obj, cost }
    });
    let log = &mut w.place_log[slot];
    log.push((req.req, res));
    if log.len() > PLACE_LOG_LEN {
        let _ = log.remove(0);
    }
    res
}
