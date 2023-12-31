let PI: f32 = 3.14159265359;

struct Uniforms {
    perspective: mat4x4<f32>;
    orthographic: mat4x4<f32>;
    total_time: f32;
    delta_time: f32;
};

[[group(0), binding(0)]]
var<uniform> r_uniforms: Uniforms;

struct VertexInput {
    [[builtin(vertex_index)]] v_index: u32;
    [[location(0)]] position: vec3<f32>;
    [[location(1)]] surface_color: vec3<f32>;
    [[location(2)]] line_color: vec3<f32>;
    [[location(3)]] intensity: f32;
    [[location(4)]] delta_intensity: f32;
};

struct VertexOutput {
    [[builtin(position)]] position: vec4<f32>;
    [[location(0)]] color: vec3<f32>;
    [[location(1)]] intensity: f32;
    [[location(2)]] delta_intensity: f32;
};

[[stage(vertex)]]
fn vs_main(
    in: VertexInput
) -> VertexOutput {
    var output: VertexOutput;
    output.position = r_uniforms.perspective * vec4<f32>(in.position, 1.0);
    output.color = in.surface_color;
    output.intensity = in.intensity;
    output.delta_intensity = in.delta_intensity;
    return output;
}

[[stage(fragment)]]
fn fs_main(
    in: VertexOutput,
) -> [[location(0)]] vec4<f32> {
    return vec4<f32>(in.color * in.intensity, in.delta_intensity);
}
