// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `PointLight` (`mclib/gosfx/pointlight.cpp`).
//!
//! In the shipped game the light manager is a stub (`LightManager::ChangeLight` does nothing),
//! so a point light never lit anything. The port computes the same values and reports them in
//! the draw list so a renderer can use them; the shipped `mc2.fx` has no point lights.

use crate::draw::{DrawInfo, DrawList, LightDraw};
use crate::effect::{Ctx, Data, Effect, ExecuteInfo};
use crate::fcurve::FCurve;
use crate::math::{Mat, Obb, Vec3};
use crate::spec::SpecKind;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LightState {
    pub active: bool,
    pub color: [f32; 3],
    pub origin: Vec3,
    pub intensity: f32,
    pub inner: f32,
    pub outer: f32,
}

impl Effect {
    /// `PointLight::Start`.
    pub(crate) fn light_start(&mut self, ctx: &mut Ctx, info: &ExecuteInfo) {
        self.effect_start(ctx, info);
        if let Data::Light(l) = &mut self.data {
            l.active = true;
        }
    }

    /// `PointLight::Execute`.
    pub(crate) fn light_execute(
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
        if let (SpecKind::PointLight(ls), Data::Light(l)) = (&spec.kind, data) {
            l.color = [
                ls.red.compute(b.age, b.seed),
                ls.green.compute(b.age, b.seed),
                ls.blue.compute(b.age, b.seed),
            ];
            l.origin = Mat::multiply(&b.local_to_parent, &info.parent_to_world).translation();
            l.intensity = ls.intensity.compute(b.age, b.seed);
            l.inner = ls.inner_radius.compute(b.age, b.seed);
            l.outer = ls.outer_radius.compute(b.age, b.seed);
        }
        true
    }

    /// `PointLight::Kill`.
    pub(crate) fn light_kill(&mut self) {
        self.effect_kill();
        if let Data::Light(l) = &mut self.data {
            l.active = false;
        }
    }

    /// The light is reported with the draw data; `PointLight` has no `Draw` in the C++.
    pub(crate) fn light_draw(&self, _info: &DrawInfo, out: &mut DrawList) {
        let (SpecKind::PointLight(ls), Data::Light(l)) = (&self.spec.kind, &self.data) else {
            return;
        };
        if l.active && self.is_executed() {
            out.lights.push(LightDraw {
                position: l.origin,
                color: l.color,
                intensity: l.intensity,
                inner_radius: l.inner,
                outer_radius: l.outer,
                two_sided: ls.two_sided,
            });
        }
    }
}
