@group(0) @binding(0) var<uniform> animation: vec4<f32>;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> Vertex {
    let points = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    let point = points[vertex_index];
    return Vertex(vec4(point, 0.0, 1.0), point);
}

@fragment
fn fs_main(vertex: Vertex) -> @location(0) vec4<f32> {
    let point = vertex.uv * vec2(1.777778, 1.0);
    let radius = length(point);
    let angle = atan2(point.y, point.x);
    let phase = animation.x;
    let field = exp(-2.7 * radius * radius);
    let bands = 0.5 + 0.5 * cos(24.0 * radius - 2.0 * phase + 2.0 * sin(3.0 * angle + phase));
    let ring = exp(-45.0 * abs(radius - 0.52 - 0.07 * cos(5.0 * angle - phase)));
    let blend = 0.5 + 0.5 * cos(2.0 * angle + phase);
    let tint = mix(vec3(0.13, 0.63, 0.94), vec3(0.92, 0.27, 0.74), blend);
    let background = vec3(0.035, 0.040, 0.075);
    return vec4(background + tint * (0.28 * field * pow(bands, 4.0) + 0.75 * ring), 1.0);
}
