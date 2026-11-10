# flock-render API

wgpu 29 renderer for Cluster Flock (SPEC section 11), forked from Stampede's race-render. Plain data in, pixels
out: **no dependency on flock-core**; the app's presenter maps sim state to the types below. Everything is
procedural (authored mesh builders, no model or image decoders). Source: `crates/flock-render`.
`cargo run -p flock-render --example farm_shot --release -- --out docs/notes/shots` writes the reference
shots (`farm_runner.png`, `farm_ghost.png`, plus `farm_lineup.png` for the art) and prints `stats()`.

## Coordinates: read this first (SPEC section 2)

Metres, right-handed, **Y up, +Z forward, yaw 0 faces +Z**. The renderer's right at yaw 0 is **-X**
(`forward = (sin yaw, 0, cos yaw)`, `right = (-cos yaw, 0, sin yaw)`). The sim's `x` is + = right, so the
presenter mirrors x, in one place:

```rust
// render space <- sim space. The ONLY place x is mirrored.
fn to_render(sim_x: f32, sim_y: f32, sim_z: f64, origin_z: f64) -> Vec3 {
    Vec3::new(-sim_x, sim_y, (sim_z - origin_z) as f32)
}
// lane l in 0..=2 (left..right): render x = -(l as f32 - 1.0) * lane_width   -> lane 0 is at +2.2
// fences: inner faces at render x = +-fence_x (3.7). Left/right of the *runner* = +x / -x.
```
Colours are `[f32; 3]` sRGB (as you would write a hex colour). Vectors are `glam` 0.29.

## Renderer

```rust
Renderer::new(device, queue, surface_format, Quality) -> Renderer
fn resize(&mut self, w, h)            // output size in pixels; call on every surface resize
fn set_quality(&mut self, Quality)    // Battery 0.7x no MSAA, half particles / Balanced 0.85x MSAA4 / Max 1.0x MSAA4
fn load_world(&mut self, device, queue, &WorldGeometry)   // bake the static world, once (about 40 ms desktop)
fn render(&mut self, device, queue, encoder, target_view, &FrameScene, dt)
fn step_particles(&mut self, &FrameScene, dt)             // advance particles without drawing (pause, menus)
fn shift_world_z(&mut self, dz: f32)                      // floating origin moved: see below
fn stats(&self) -> RenderStats                            // { draw_calls, triangles, particles } of the last render
```
`render` clears and fully covers the target; open egui's pass afterwards with `LoadOp::Load`. `dt` only drives
particles (0 freezes them). One `render` per `queue.submit`. `render` and `step_particles` both advance
particles: call one of them per frame. `RUST_LOG=flock_render=debug` prints stats per frame.

## The farm world and the floating origin

```rust
renderer.load_world(&device, &queue, &WorldGeometry::farm());     // or farm_world(FarmStyle { seed, lane_width, fence_x })
flock_render::TILE_LEN                                              // 240.0 m
```
A straight three-lane dirt track with ruts and dashed lane marks, wooden fences at `+-fence_x`, mown grass
verges with flowers and bushes, crop fields (wheat, corn, sunflowers, cabbage, lavender), two red barns, silos, a
farmhouse, two windmills (the wheels turn), haystacks, bale piles, scarecrows, trees, ponds, sheep that hop,
cows, and rolling hills plus a bright sky. The track is centred on x = 0 and runs along +Z. One **tile** is
`TILE_LEN` long and repeats seamlessly; `load_world` bakes it four times (z from `-TILE_LEN` to
`3 * TILE_LEN`), culled per 64 m chunk, about 44k triangles per tile (about 60k drawn at once).

**The app keeps render-space z small** (f32 precision, and the baked range is finite). Every frame:

```rust
let t = TILE_LEN as f64;
let origin = (anchor_z / t).floor() * t;                 // f64, a multiple of TILE_LEN
if origin != last_origin { renderer.shift_world_z((last_origin - origin) as f32); last_origin = origin; }
let rz = |sim_z: f64| (sim_z - origin) as f32;           // for EVERY z handed to the renderer
```
Then the runner camera's z stays in `[-9.0, TILE_LEN - 9.0)`, which always has scenery behind and about two
tiles ahead. The tile has no visible seam, so the origin may change at any moment (it does not need to be
hidden). Particles in flight are shifted by `shift_world_z`; skip it and anything alive at that instant jumps
away (they live at most about two seconds). Send only objects within about 250 m of the camera. Because `z`
is relative, runners behind the anchor by a few metres are simply at slightly negative render z at the wrap.

## Cameras (the app's job; these are the SPEC recipes)

```rust
// Runner camera, behind and above the pack anchor (render-space anchor z). aspect = width / height.
Camera::runner(anchor_z, aspect)
// = Camera { eye: (0, 7.6, anchor_z - 9.0), target: (0, 0.0, anchor_z + 10.5), up: Y, fov_y: <below>, near: 0.4, far: 230.0 }

// Ghost camera, higher and diagonal, looking down the placement band. Sim x -5 is render x +5.
Camera::ghost(anchor_z)
// = Camera { eye: (5.0, 15.0, anchor_z - 3.0), target: (0.0, 0.0, anchor_z + 40.0), up: Y, fov_y: 58 deg, near: 0.4, far: 210.0 }
```
**Field of view matters on a portrait phone.** SPEC says about 62 degrees vertical, but on a 9:20 screen that
is only a 31 degree horizontal field: at the pack anchor (about 10 m from the lens) it shows 5 m of width, so
birds in the outer lanes are cut off. `Camera::runner` raises `fov_y` (to about 70 degrees on a 1080x2400
screen) so the three lanes and the runners' bodies fit at the anchor's depth; it stays between 62 and 84 degrees.
Build your own `Camera` if you prefer, but keep `near` at 0.4 or more (the ground decals sit a few centimetres
above the ground and need the depth precision). Ease the camera yourself; `FrameScene::shake` offsets eye and
target, `FrameScene::fog_scale` scales the fog (1 = as authored; the farm fog hides things beyond about
200 m).

## FrameScene

`FrameScene { camera, time, ostriches, coins, farm_props, placement, emitters, bursts, shake, fog_scale, .. }`
(`Default` = empty). `time` is a monotonic animation clock in seconds (wing and tail flutter, windmills,
cloud drift, coin spin is yours). Fields kept from Stampede and unused by Flock (leave them empty):
`frogs roses rugs targets projectiles hazards drill_props beams shields mirror start_lights`.

`CoinInst { pos (centre), spin (radians), visible }`: the existing gold coin (radius about 0.35 at scale 1).

### OstrichPose: one Nostrich

`OstrichPose::for_player(i)` gives player `i`'s colours and accessory (`PlayerStyle::of(i)`: Tomato/Crown,
Sky/Cap, Lemon/Viking horns, Plum/Mohawk, Mint/Star headband, Bubblegum/Scarf: six saturated colours that also
differ by head accessory so players read without colour). `Default` is a plain bird with **no jockey** (rider,
saddle, laptop, cake, rugs and printer fields still exist in the struct for the old code path; leave them at
their defaults and nothing of the jockey is ever drawn).

| field | meaning | drive it from |
|---|---|---|
| `pos` | ground point under the body (render space, mirrored x), `pos.y` = height above ground | sim x, y, z |
| `yaw` | 0 = running down the course | 0 (or a small lurch while changing lane) |
| `speed` | m/s, only for stride and neck stretch | sim `vz` |
| `gait_phase` | 1.0 = two steps; only the fraction matters | `+= dt * speed / (1.8 + 4.2 * clamp(speed / 32, 0, 1))` keeps feet planted; a slower rate looks floppier |
| `squash` 0..1 | wind-up crouch: body flattens, knees fold | cosmetic anticipation from the command (SPEC section 0), ease 0 to 1 over `jump_windup` / `down_windup`, release on take-off |
| `neck_sway` (x, y) | floppy neck: x + = runner's right, y + = forward, about -1..1, grows towards the head | the presenter's spring output (lateral acceleration, braking) |
| `body_roll` (rad) | upper-body roll on top of `lean`, + = right | spring output |
| `body_pitch` (rad) | upper-body pitch, + = nose down | spring output (negative on take-off, positive on landing) |
| `lean` (rad) | whole-bird roll, + = right | lane-change lurch |
| `tumble` (rad) | full-body roll about the heading axis through the body centre; the bird is lifted so nothing digs in | tumbling: ramp 0 to +-2 pi (sign = which way); 0 and 2 pi are both upright |
| `ball` 0..1 | **tuck-and-roll** (the crouch): a feathery ball, neck curled down against the chest, legs tucked, wings wrapped; the body centre sits at `BALL_RADIUS` (0.58 m) so the ball rests on the ground | `Collapsed` (ease in about 0.05 s, out about 0.07 s during `Recovering`); set `collapsed` to 0 |
| `roll` (rad) | somersault of the ball about its own centre, + = forward; **angle = distance rolled / `BALL_RADIUS`** (whole turns are upright) | presenter accumulator: `+= speed * dt / BALL_RADIUS` while `Collapsed`; on stand-up ease it to the next multiple of 2 pi and reset to 0 when `ball` has unfurled. Puff `BurstKind::DustTrail` each half turn |
| `collapsed` 0..1 | old belly-flop slide (body on the ground, legs out behind, wings wide). No longer used by the presenter; the renderer still draws it | leave 0 |
| `wing_flap` 0..1 | wings beat | 1 while airborne or panicking |
| `scramble` 0..1 | legs pedal wildly, independent of the ground | airborne, involuntary hops, tumbling, pile-ups |
| `tumble` is still the involuntary comic tumble (Tumbling); it is independent of `ball`/`roll` |
| `airborne`, `tuck` | jump pose: wings out, legs tucked by `tuck` 0..1 | sim airborne |
| `stars` 0..1 | five stars circle the head (scales in) | protected |
| `ghost` 0..1 | translucent, pale, floating about 1 m up, wavy tail instead of legs, bright rim in the player's colour | eliminated: ease 0 to 1 over about 0.4 s |
| `outline` | `Some(colour)`: a clear rim around the body, neck and legs | the local player only |
| `body neck legs beak accent accessory` | colours (sRGB) and `Accessory` | `PlayerStyle::of(i)` |
| `scale`, `look_back`, `bash`, `spin`, `trick_flip`, `wheel`, `trumping`, `laser_on`, `burning` | inherited effects | leave default |

Helper functions: `ostrich_parts(&pose, time)`, `head_transform(&pose)`, `eye_positions`, `foot_positions`.
Ghost birds draw through the translucent pass (blended, depth-tested, no depth write, after the opaque
meshes). Eyes are big googly eyes whose lids carry the mood: half-lidded while running, wide open in the air,
squeezed in the wind-up and in the tucked ball, closed on the belly-flop, spinning when dizzy (tumble, stars).

### FarmPropInst: obstacles, previews, reservations

```rust
FarmPropInst { kind, pos, size, alpha, drop, danger }      // FarmPropInst::new(kind, pos, size) = solid, landed
FarmPropKind::HayBale                                      // size (1.6, 0.7, 1.0): STATIONARY rectangular bale (two stacked, red twine); it never rolls
FarmPropKind::Beam { span_lanes: 1 | 3 }                   // size (span, underside 1.15, depth 0.4) + bar_top (sim Obj::top, e.g. 1.95): overhead farm gate
FarmPropKind::Barrel                                       // size (width across the lane, diameter, diameter) + roll: a lying wooden barrel that rolls
FarmPropKind::Block { variant: BarnWall | Tractor | Trough }  // size (2.0, 2.5, 3.0): the face the runners see is -z
FarmPropKind::Mud                                          // size (width, ignored, depth 6.0): a glossy decal on the ground
```
`pos` is the **centre of the footprint on the ground** (render space; a 3-lane beam has `pos.x = 0`). `size` is
the full width, height and depth from the sim; the meshes are stretched to it, so use the sim numbers (the
meshes are drawn for those reference sizes). Hay, barrels, beams and blocks get a soft ground shadow.

**Beam = overhead gate** (the thing you crouch under; it must never read as a hurdle). Pass `size = (2 * half_w,
Obj::y, 2 * half_d)` (`y` = underside) and `bar_top = Obj::top`. The renderer draws tall posts (about 2.6 m) on stone
footings with a top rail and spindles, a thick yellow/black striped bar exactly from the underside up to `bar_top`
(blue "duck" arrow boards, one per lane), bunting and feed sacks dangling under it, an open gap below and a dark
shadow line on the ground under the bar. The frame only grows if `bar_top` is raised above about 2.0 m; it is
drawn as two instances (frame + bar). `bar_top = 0` means underside + 0.6.

**Barrel**: `size = (2 * half_w, Obj::top, 2 * half_d)`, `roll = Obj::roll` (radians about x, + = rolling towards +z,
away from the runner camera; negative = towards it; `roll = travel / radius` so it matches the ground). The
staves are two-tone with two red staves, so the spin shows. The renderer adds the shadow only: spawn the dust
yourself, e.g. `Burst::new(BurstKind::DustTrail, barrel_pos + Vec3::Z * (behind * radius), 4)` every 1.2 m rolled,
`behind = -dir`.

* `alpha` below 1 draws the prop translucent (a reservation: accepted but not yet solid). Use about 0.55.
* `drop` 0..1 is the architect's drop-in: 0 is 9 m up and invisible, it falls with ease-in until 0.85, squashes
  on landing and springs back by 1.0. Drive it with `materialise_s`: `drop = elapsed / materialise_s`.
* `danger = true` tints it red and pulses it (an invalid preview). Combine with `alpha` (about 0.6).

### PlacementCell: the ghost's ground grid

```rust
PlacementCell { pos, size: Vec2, state }     // pos = cell centre on the ground, size = (width x, depth z)
CellState::Valid (green) | Invalid (red) | Reserved (violet) | Selected (yellow, pulses)
```
One per lane x row in the band (e.g. `size = (lane_width - 0.2, row_spacing - 0.4)`); each draws a translucent
fill and a frame 14 cm above the ground. Cells and previews go in the translucent pass (depth-tested), so
solid props in front of them hide them correctly.

### Particles

Per frame, one `Emitter { racer (index into ostriches), dust 0..1+, dust_color, offroad, .. }` per grounded
runner gives **feet dust** (`dust` from speed or slide; `offroad: true` makes bigger clouds, e.g. in mud; use
the track colour about `[0.78, 0.6, 0.38]`). One-shot bursts go in `FrameScene::bursts` for exactly one frame:

| `Burst::new(kind, pos, count)` | use | notes |
|---|---|---|
| `Feathers` + `.with_color(body)` | crash (feather lost) | feathers in the runner's colour (a quarter white) flutter off and sink; a puff of down. `count` 12..24 |
| `HitPuff` | any hard impact | |
| `MudSplat` | first mud overlap | glossy brown droplets thrown up and a dirty puff. `count` about 20 |
| `CoinSparkle` | coin collected | at the coin |
| `LandingDust` | landing | ring of dust at the feet. `count` about 24 |
| `DustTrail` | rolling barrel, the roll's thump | a few small low puffs that linger. `count` 4 to 6 |
| `GhostPoof` + `.with_color(body)` | elimination | pale ring, sparkles, wisp; put it where the runner was |

The pool is 3600 (1800 on Battery); when full the oldest are recycled.

## Budgets

Reference scene (1080x2400, Max, MSAA 4x, Intel Iris Xe; the numbers do not depend on resolution): the runner
shot (7 birds incl. the ghost, local outline, 8 props, 13 coins, 279 particles) is **61 draw calls and about
101k triangles**: 61k world, 41k birds, props and coins. The ghost shot (placement grid, 7 previews and props)
is 60 draws, 93k triangles. A bird is about 4k triangles (budget test: under 5500 with accessory and laser).
The world is chunked in 64 m cells with frustum and distance culling, adjacent visible chunks merged into one
draw; about 13 world draws. Translucent and outline passes add a handful of draws (one per mesh kind).
Battery drops particles to half and the render scale to 0.7. Pixel 8 / OnePlus 8T: the shaders are the
Stampede ones (flat lighting, a single fetch for dust); the only new fill cost is the translucent ghost and
cell pass and the outline hull of one bird, so Stampede's measured on-device numbers still apply; the first
Flock on-device check should look at the dust fill with six emitters at `dust` 1.

## Art and materials

Procedural low-poly meshes with flat or smooth shading, wrapped sunlight, hemisphere light and a soft rim; blob
shadows under every bird and prop; sheen on mud, water and beaks. The sky is a shader (blue gradient, clouds,
two layers of rolling hills) and the farm fog matches the horizon colour. Windmill wheels are instanced each
frame; sheep and pennants animate in the vertex shader (no CPU cost).

Each bird: a plump smooth body with overlapping feather scallops (tinted by `body`), a fan tail and fat little
wings (tips in `accent`), long wobbly neck (`neck`), big head with googly eyes and a beak, big comic feet
(`legs`). The local outline is an inverted hull in the chosen colour on the smooth parts (not the plumes).

## GLES 3 notes

Written to the GLES 3 feature set by construction: no storage buffers, no dynamic uniform offsets, no base-vertex
or first-instance draws (indices are absolute, per-draw instance data is a sliced vertex buffer), no
`instance_index`, uniform structs are vec4-aligned, 9 vertex attributes, Depth24Plus, Rgba8Unorm scene targets
with a 4x MSAA resolve. The new translucent, ghost and outline pipelines use the same vertex layout and
bindings. **Verified** on Vulkan and on wgpu's GL backend (`WGPU_BACKEND=gl`, desktop OpenGL via Mesa) and at all
three quality tiers (identical output). **Not yet run** on a real ES 3 context or Adreno.

## Notes for the app

* `render` and `step_particles` both advance particles; call one per frame.
* `Renderer::set_diag(Diag)` (measurement only) toggles `world`, `meshes`, `shadows`, `dust`, `additive`,
  `sky`, `blit`, `scale`, `msaa` and `repeat=N`; `Diag::default()` changes nothing.
* The baked world is not rebuilt when props change; props, cells, birds and coins are instanced each frame.
* Legacy Stampede scenery, road and terrain types still exist in `WorldGeometry`; set `farm: None` for them.
