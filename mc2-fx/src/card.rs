// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Card` (`mclib/gosfx/card.cpp`): one camera-facing, texture-animated quad.

use crate::cloud::{card_corners, push_quad};
use crate::draw::{DrawInfo, DrawList};
use crate::effect::{Ctx, Data, Effect, ExecuteInfo};
use crate::fcurve::FCurve;
use crate::math::{truncate_float_to_byte, Obb};
use crate::singleton::{align_to_camera, singleton_animate, SingletonState};
use crate::spec::SpecKind;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CardState {
    pub s: SingletonState,
    pub half_x: f32,
    pub half_y: f32,
    pub uv: [[f32; 2]; 4],
}

impl Effect {
    /// `Card::Start`.
    pub(crate) fn card_start(&mut self, ctx: &mut Ctx, info: &ExecuteInfo) {
        self.effect_start(ctx, info);
        let Effect { spec, b, data } = self;
        if let (SpecKind::Card(cs), Data::Card(st)) = (&spec.kind, data) {
            st.half_y = cs.half_height.compute(b.age, b.seed);
            st.half_x = st.half_y * cs.aspect_ratio.compute(b.age, b.seed);
            st.s.radius = (st.half_x * st.half_x + st.half_y * st.half_y).sqrt();
        }
    }

    /// `Card::Execute`.
    pub(crate) fn card_execute(
        &mut self,
        ctx: &mut Ctx,
        info: &ExecuteInfo,
        bounds: &mut Obb,
    ) -> bool {
        if !self.is_executed() {
            return false;
        }
        if !self.effect_execute(ctx, info, bounds) {
            return false;
        }
        let Effect { spec, b, data } = self;
        let (SpecKind::Card(cs), Data::Card(st)) = (&spec.kind, data) else {
            return true;
        };
        singleton_animate(
            &cs.singleton,
            b.age,
            b.seed,
            &mut st.s,
            &b.local_to_parent,
            bounds,
        );

        // Animate the UVs.
        let uv = &cs.uv;
        let mut u = uv.u_offset.value;
        let mut v = uv.v_offset.value;
        let mut u2 = uv.u_size.value;
        let mut v2 = uv.v_size.value;
        if uv.animated {
            let mut columns = truncate_float_to_byte(cs.index.compute(b.age, b.seed));
            let rows = columns.checked_div(uv.width).unwrap_or(0);
            columns = columns.wrapping_sub(rows.wrapping_mul(uv.width));
            u += u2 * columns as f32;
            v += v2 * rows as f32;
        }
        u2 += u;
        v2 += v;
        st.uv = [[u, v2], [u2, v2], [u2, v], [u, v]];
        true
    }

    /// `Card::Kill`.
    pub(crate) fn card_kill(&mut self) {
        self.effect_kill();
    }

    /// `Card::Draw`.
    pub(crate) fn card_draw(&self, info: &DrawInfo, out: &mut DrawList) {
        let (SpecKind::Card(cs), Data::Card(st)) = (&self.spec.kind, &self.data) else {
            return;
        };
        // `Card::Kill` turns the card off at once (`MLRCardCloud::TurnOff(0)`).
        if !self.is_executed() {
            return;
        }
        let (state, mut local_to_world) = self.draw_frame(info);
        align_to_camera(&cs.singleton, &mut local_to_world, &info.camera_to_world);
        let corners =
            card_corners(st.s.scale, st.half_x, st.half_y).map(|q| local_to_world.point(q));
        let c = st.s.color.to_array();
        push_quad(out, &state, corners, st.uv, [c; 4]);
    }
}
