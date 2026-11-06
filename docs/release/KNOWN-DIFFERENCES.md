# Known differences from the original game (0.1.0)

MC2 Rust is meant to play like MechCommander 2 without reproducing the original's arithmetic exactly. This page lists the differences a
player could notice, in plain language. The D-numbers below are the numbers of the project's full difference register (with the original code locations and the tests that
pin each behaviour), which is not part of this repository. Entries with no D-number there are named by their
section heading.

## Not ported

**D-22: single player only.** Multiplayer, the camera drone and the elemental units are not in the port.

**Radio messages (first entry of the register).** A pilot's radio message is voice only. The mission chat line shows only the text a mission script sends (and the tutorial's text); the port no longer adds a line per radio event, as earlier Rust builds did.

**D-24: no debug cheats.** The original's debug-only keyboard entries and debug frame-rate hot keys are not ported.

**D-85: no UI scale setting.** The original has none either, but earlier Rust builds had one. It is removed; the screens fit the window.

**D-69: support aircraft are not drawn in full.** The support VTOL's drop, departure and timing follow the data, but the VTOL model, its dust and the
recovery beam are not drawn. The recovery craft hovers over the mech's position rather than over its cockpit.

**Airstrike bomber (section "Airstrike bomber flyover and contrail").** The bomber and its contrail are drawn; the bomber's own sound and
shadow are not.

## Graphics

**Upscaled art (section "Upscaled art by default").** The original draws its textures at their authored size. On the first start the game builds a 2x
upscaled texture pack from your own data and uses it. `--hires-dir off` restores the original pixels. Paint areas stay pure colour and
cut-out edges stay hard.

**D-107: paint colours.** Mechs and vehicles are painted as the original Windows build paints them: the shipped preferences give a yellow
base with dark-blue highlights. The Linux community build that the port was compared against shows the same preferences as teal with dark red
(red and blue swapped), so screenshots from that build look different from this one.

**D-86 (screen details):** the salvage panel prints a mech's weight as whole tons (the original prints "55.000000"); the pilot review draws kill
markers as the killed units' icons with their armour colours at the end of the mission, built once when the review starts; the slide-in
animations of the solo mission list are absent. A mech's 3D preview can be empty for its first frame after you pick it. The salvage 3D view shows the wreck's own paint scheme (or your base colour); which of the three colours goes where was reasoned from the code, not compared with a reference picture.

**Mech bay 3D picture: brighter (no D-number; measured for 0.1.0).** The turning 3D picture of the picked mech on the mech bay and
purchase screens is drawn brighter than the original: compared with a capture of the original at 1600x1200, its mean brightness is about
5 to 16 percent higher depending on the paint scheme, and the mech can sit one or two pixels off the original's framing.

**D-78 and D-87:** the results screens and the mech purchase list follow the original's layout. D-78 lists presentation differences in the
results screens (see D-86 above); D-87 records two original quirks: one is not copied (a refused mech purchase no longer leaves a row in the owned list; the original added one anyway) and one is copied (a building that shares a name with another is shown with the first one's data, as in the original).

**Craters and footprints (section "Persistent craters and footprints").** The original's crater picture table is indexed wrongly, so every
crater drew the first crater picture. The port draws the four crater pictures and the footprint pictures the table was meant to select.
Footprints are drawn only for a mech the camera sees, and the decals are drawn slightly above the ground.

**D-23: explosions end on their hit frame in the simulation.** The visual explosions have their own lifetimes; weapon bolts keep their post-hit
life because it can change kills.

**Weapon and combat effects (section "P3 weapon and combat effects").** Effects start on the draw clock, a fraction of a frame from the simulation event. The tint that flashes a building when it is hit is not shown (it is counted and logged once). A miss over water starts the water splash effect at the water level.

**D-94: bolt effects.** A weapon bolt's in-flight trail or beam is wound down when the bolt ends instead of a fixed time later.

**D-105: queued waypoint markers.** Markers for queued orders sit on the ground where you clicked. Path lines are two pixels wide, not one.

**D-42: animated buttons.** A scaled animated button grows about its centre and animated objects return to their exact size afterwards (the original resized wrongly and an object grew a half pixel every frame).

**D-44: mech list.** The size of each entry in the mech list comes from the layout's first rectangle, because the original read a missing block.

**D-NIGHT-SHAPE: night light on buildings and mechs.** The glow of night lights on shapes is sampled from the same ground light map as the
terrain, so a point high above the ground is lit slightly differently (a few percent at building height).

**D-CALLOUT-POINTER: tutorial pointer.** During a tutorial call-out the game draws its own pointer gliding to the named button. The real
mouse pointer is not moved, though the game treats it as being on the button until you move the mouse.

**Appearance-attached effects (section of that name).** Foot dust, wakes and vehicle dust are hidden while the original hides them and start
again when a vehicle starts moving after standing still. Helicopter dust is placed at the helicopter instead of at its drop origin. Ground
vehicles now play their gestures (for example the repair truck's), which were unreachable in earlier Rust builds. These effects are not saved;
loading a save restarts a mech's smoke at its saved level.

## Weather

Rain, lightning, thunder and darkening follow the mission's weather block, but the random sequence is not the original's, so a given mission
does not rain at the same moments.

**D-57, D-95, D-96: when it rains.** The original lets the rain level go below zero when rain stops (so it could never rain again), re-checks
every frame once dry, and never rolls for rain in a mission that starts dry. The port keeps the level at zero, checks every 30 seconds, and
rolls the mission's `ChanceOfRain` also when the sky is dry.

**D-58: lightning chance.** The mission's `BaseLighteningChance` is used.

**D-59, D-61, D-63: light and fog.** The light fades back when rain stops. Terrain darkening is a separate multiplier. Fog is not darkened by
weather.

**D-60: lightning flash.** Terrain adds the flash after the texture; shapes brighten their ambient light.

**D-62, D-64: rain streaks.** Streaks are one-pixel-wide quads, and fade with linear depth.

## Gameplay and combat

**D-06: first critical hit.** The original leaves a component's disable level unset. The port uses zero, so a component is disabled by its first critical hit.

**D-19: silent radio samples.** A radio sample that is missing or not a sound file is silent; the message simply ends. The support-unit voice sets have many empty samples (29 of 40 packets in `pilot1a` to `pilot1d`), so some support calls have no voice. The original could crash there.

**D-29, D-56 and escape routes: path choices.** The last leg of a long route stays inside the areas the long route crosses. When routes cost the same, including the escape search a mech uses to leave a bad tile, the port may take a different one than the original. Units may therefore take a different path, to the same outcome.

**D-33: single missions.** A single mission starts with logistics enabled and is named after its map. The original left the logistics flag unset.

**D-41: text hit tests.** A text item tests the point it is given. The original read the mouse position instead of its argument.

**D-49: objects at the screen edge.** Whether a building or unit is counted as on screen can differ from the original only for small selection boxes lying wholly inside the outer 96 pixels of the view. Positions, damage and objectives are not affected.

**D-52: destroyed mechs.** A mech destroyed while standing falls and then shows its shipped fall-damage model. The original delayed this
and the mech fell with its intact model.

**D-72: group visibility in mission scripts.** When a script asks whether a group of units can see a target, the port checks each living member and answers yes if one sees it. The original read an unassigned pointer in that case and could crash, so script outcomes that depend on it can differ.

**Turret attacker.** When a turret fires at a unit, the target's pilot (and any script reading the attacker) now sees the turret as the attacker. The original passed the wrong kind of identifier, so the turret was never identified.

**Vehicle attack-point orders.** A ground vehicle ordered to attack a point now checks line of sight to that point. The original checked a missing object, so the vehicle never saw the point and kept asking for a new path.

**Support helicopters and ground vehicles (section "Appearance-attached effects").** The support drop treats every air-type helicopter, including the Pegasus scout, as starting landed; the repair truck's work animations and effect are reachable; helicopter dust is placed at the aircraft's position. Dust and wake effects that were skipped in the original are hidden and resumed as the original's conditions change. Effect state is not saved, so an effect may restart after a load.

**Artillery strike slots (section "Deferrals").** A new strike that would reuse the slot of a strike that has just landed is refused, because the original holds that slot until the hit effect ends and that effect's lifetime is not ported. A sensor strike is refused if the sensor pool is full (the original stops with an error there).

**Artillery sensor strike radius.** The reveal radius of a sensor strike starts 0.2 % larger than the data's nominal figure, which is what the original's own constants give; the port keeps it.

**D-56: jump routes.** When several jump routes cost the same, the port may choose a different one than the original.

**NO-LOF-BESTBEST: attack positions.** When a mech cannot see its target and the first candidate position has no route, the original keeps a position it cannot reach. The port picks the cheapest reachable candidate, so some enemy and player mechs close on a target by a different route than the original.

**JUMP-EDGE-CELLS: jumping enemy mechs.** The community Linux build never lets a jump-capable mech enter the first row or column of the map window, so a mech whose goal door lay there never moved. The port keeps the original Windows rule: the mech walks to the door.

**D-65: zero-distance jump.** A jump to the mech's own position plays the jump animation in place and lands where it started (the C++ computed a NaN position).

**D-74: relative positions.** Orders that use a position relative to a point check passability with the right coordinate (the C++ left one unset), so the chosen destination can differ.

**Vehicle weapons.** A ground vehicle fires from every one of its weapon nodes; in the original a vehicle with three or four weapon nodes stopped firing once the rotation reached the third. A vehicle given an attack-a-point order now sees the point and goes there; the original kept re-requesting a path and never moved.

**D-101: jump landings.** Landing on a mech or vehicle deals the original's damage, rotates and bounces the other unit as in the original. Not reproduced: the ram and death-from-above mission statistics, and the toss-and-spin of a vehicle wreck crushed by a landing.

**D-106: vehicle crews.** When a vehicle's crew bails out, the crew stays on the field while the camera sees it (the original removed it on the frame
it appeared), is selectable like other units, and is not removed on the frame it is created.

**D-80: wiped-out force.** If your whole force is dead or disabled, the mission is lost even if a mission script would otherwise make an
objective count as success (mc2_21 in the original could be won with no mech alive).

**D-73 and D-100: script-created infantry.** Infantry created by a mission script get the shipped infantry brain. That script does not compile as shipped (an undeclared variable, a `patrol` call with the wrong arguments, a missing `endorder`), so the port repairs it when it loads it. D-100 in the register ("no brain yet") is superseded by this repair. Mission-script infantry therefore move and attack on their own.

**D-73 (team changes):** a script that changes a unit or group's side now takes effect; the original stopped with a fatal error there.

**D-65b: Recover order.** A recovery vehicle drives to the cell beside the disabled mech, not to the mech itself, and ends up within the same
recovery range.

**D-66: Recover order.** In the original, the recovery vehicle's Recover order ends in code that stops with "RECOVER TACORDER NO LONGER VALID" (the working sequence is compiled out). In the port the recovery vehicle recovers the disabled mech: the mech is traded to your side with the best pilot your campaign has, repaired and powered up, and the recovery vehicle is used up. If your campaign has no pilot to give, nothing changes (see D-70).

**D-70: recovery with no pilot.** If the roster has no pilot to give, the recovery is refused, the mech stays as it was and the cost is returned.
The original crashed there.

**D-67: mine sweepers.** A sweeper stops when its front armour reaches zero or below, instead of only at exactly zero.

**D-68: salvage line of sight.** Whether salvage is offered for a picked object depends on your team having visual contact with it.

**D-99: strike hot keys.** The airstrike and sensor-strike hot keys cannot press a disabled strike button, so they no longer spend the cost when the mission or your resource points forbid the strike.

**Called shots (section "mc2-app player-order bridge").** A click on a mech whose arms or head are already destroyed no longer issues an aimed shot at the
missing part.

**D-48: unit bar health.** The health bar under each unit icon uses leg armour for the leg term; the original read arm armour by mistake.

**D-43, D-46, D-88 (force groups):** a dead unit's icon leaves its group on the same frame, groups of equal pilot names keep a stable order,
and a destroyed member leaves its group without shifting the others wrongly. (The second D-88 in the register is retired: a knockdown from a
failed landing check is dropped, as in the original.)

**D-34, D-32: mech lab.** Renaming a variant to a name an inventory mech's variant already has is refused (the original deleted the other
variant), and a variant holds at most 54 components.

**D-45: pilot review.** A promoted pilot with no specialty left to choose no longer holds up the review (the original could not leave that screen).

**Tactical map objective flash.** An objective call-out on the tactical map flashes every time it is raised; in the original, after the first completed flash no later one could start.

**In-mission results frame.** The results panel settles on the last row of its slide, scaled to the window, and opens the objective list when the slide reaches it; the original settled on the wrong row, unscaled.

## Menus, saving and pausing

**D-28: save format.** Saves use the port's own format. Saves from the original game cannot be loaded, and saves from this port cannot be loaded by the original.

**D-51 and D-55: saves.** Campaign saves are numbered slots (`slot-NN.ron`), named saves, and one autosave, in the campaign save directory.
The saves use the port's own format (D-28) and not the original's file names or mission-named autosave. The save and load picker is smaller
than the original's: it does not show mission art, save time or localised names. Selecting an existing name asks before overwriting.

**D-81: in-mission save.** SAVE and LOAD in the pause menu write and read `testgame.mc2s` in the campaign save directory, a new format. Both
are refused in single missions, as in the original.

**D-82: restart.** The pause menu's RESTART asks for confirmation and then starts the same mission again. The original ended the mission as lost.

**D-83: pause buttons.** The pause panel's buttons stay on screen at resolutions of 1280x1024 and above.

**D-84: solo mission list.** Each map is listed once; the original listed a played campaign mission twice.

**D-90: options.** All options-screen settings apply and are saved to `options.cfg`. Resolution takes effect at once and is used at the next
start when you give no `--size`. Async Mouse chooses whether the game cursor is drawn at the latest pointer position. Hardware Rasterizer set
to anything but hardware asks for a software graphics adapter and restarts the game to apply it (a mission in progress ends). Local Shadows
lets the night lights cast shape shadows.

**New Campaign list (section "Tutorial objectives are evaluated…").** The list shows the two shipped campaigns only, and its Delete button is greyed out.

**Main menu intro (section "Main menu: Escape skips the whole intro cover").** Escape during the start-up intro skips the whole black cover at
once; the original played it out.

**D-89, D-92: interface sounds.** Pointer highlight and click sounds, the unit-bar selection sound and the mission timer's warnings are
decided per simulation tick. A control-panel click sound plays only if you press and release on the same button without dragging.

## Movies

**D-91, D-93: mission-select movie controls.** Pause toggles between held and playing, and Play restarts the movie and resumes it even if it
was held (in the original a held movie stayed frozen).

**D-53, D-54: final video and pause.** A stop request ends the final video as a skip does. Pausing a movie also holds its sound.

**D-98, D-75, D-102: script movies and fades.** A movie that a script ends in its opening frame closes properly, a fade advances on the frame
its movie ends, and `FadeToColor` keeps fractional seconds (the original truncated 2.5 s to 2 s).

**Movie playback (section "Movie playback").** The first decoded picture is skipped when its timestamp is zero (it was start-up noise in the
capture checked), the playback clock starts at the first picture, and the audio is decoded when the movie opens, which can make opening a
movie take longer and use more memory. Colour conversion is done on the CPU, so single colour values can differ slightly from the original's.

**D-02: random number with a range of 0.** The original divides by zero when the game asks for a random number in a range of 0 (the turret
idle-position offset and the drop-zone placement can do this). The port returns 0 and uses up one draw, so those calls give a defined result
instead of undefined behaviour; the exact turret idle offsets or drop-zone choices in such a case can differ from what the original happened to produce.

**D-03: ties in target and order choices.** When two candidates (contacts, units within a radius, weapon candidates, group order) rank equally,
the port keeps them in the order they were added. The original's C library sort broke ties differently on each platform. Which of two equally
ranked targets is picked can therefore differ from the original, and this changes combat outcomes: in the mc2_01 comparison, fighting diverged
after about 1000 frames when the sort changed.

**D-07: routes and map reads that are out of bounds.** A route longer than 200 steps, or a long-range route over more than 50 areas, is
refused with an error, so the unit does not move (the original overran its arrays or stopped on an assert). A route that starts outside the
map is refused in the same way. Reads outside the map count as blocked or empty and writes there are ignored. Move orders that need such
a long route may therefore do nothing where the original misbehaved.

**D-104: scripts that raise alarms inside other scripts.** When a script action raises an alarm handler, the handler runs as soon as that
action returns, with its own object and warrior context; the original cleared those values after the inner call and so disturbed the outer
script. Nested script runs are limited to 64 per outermost script and a depth of 8 (the original had no limit and would overflow its stack);
requests beyond the limit are reported and not run. When a mech is disabled, the incapacitation handler sees it before it is marked disabled,
as in the original. No shipped script is known to reach the limits.

## Differences that are internal

Every other D-number in the full register, with the reason a player would not see it. Every D-number in that file is either described above or in this table. Unnumbered register entries are described above by their section name, except the internal ones named at the end. The register's "Open" section lists original defects the port still copies; it is not a list of differences.

| D-number | internal because |
|---|---|
| D-01 | two random number streams; draw order is not observable |
| D-04 | simulation clock source; same game time |
| D-05 | script random seed source; seeds are not observable |
| D-08 | float-to-integer overflow no game caller reaches |
| D-09 | empty string literal; no shipped script uses it |
| D-10 | unterminated script comment reports an error; shipped scripts are fine |
| D-11 | division by zero in scripts gives 0, as the original release build |
| D-12 | script `concat` and call-depth limits; no shipped script reaches them |
| D-13 | script debugger and profile log not ported; debug tooling |
| D-15 | compression of empty input; file tooling |
| D-16 | archive and file-name hardening against malformed input |
| D-17 | data-file parser hardening against malformed input |
| D-18 | object indices above 1023; the original treats them as fatal |
| D-21 | disabled build options; the port follows the compiled game, as the original |
| D-25 | memo table size; no visible effect |
| D-27 | path-map window shape; internal data structure |
| D-30 | out-of-range index guard; no shipped data reaches it |
| D-31 | out-of-range index guard; no shipped data reaches it |
| D-35 | pilots keyed by file name internally; displayed names unchanged |
| D-36 | path cost overflow clamp; unreachable speeds |
| D-37 | limits on malformed scripts; shipped scripts are far below them |
| D-38 | error text for a malformed script |
| D-39 | loader errors instead of crashes on bad data |
| D-40 | camera loop guard against bad data |
| D-47 | turn-1 visibility on a handful of vertices; no later state depends on it |
| D-50 | maths library last-bit differences |
| D-63 | no difference: fog is not changed by weather, in either build |
| D-71 | resolved; no difference left |
| D-97 | frame at which a stale script target is dropped; outcomes unchanged |
| D-103 | turret update without a view; headless runs only |
| D-14, D-20, D-26 | retired ids, not differences |
| ORACLE-HEADLESS-AUDIO | test-oracle limitation; the shipped game is unchanged |
| "Mission select: funds are written…" | guards a placeholder no shipped campaign state reaches |
| "Tutorial objectives are evaluated by the sim…" | same visible behaviour; only where the code runs |
| Lane D (sim-d) additions: zero tonnage, null order target, zero threat | cases no shipped data reaches; the original crashed or read garbage |
