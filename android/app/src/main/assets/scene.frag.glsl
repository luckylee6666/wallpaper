precision highp float;

varying vec2 vUv;
uniform float uTime;
uniform vec2 uRes;
uniform float uAudio; // 0..1，P1 由麦克风频谱驱动

// ── 2D simplex noise（移植自桌面版，GLSL ES 通用）──
vec3 mod289(vec3 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec2 mod289(vec2 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec3 permute(vec3 x) { return mod289(((x * 34.0) + 1.0) * x); }
float snoise(vec2 v) {
    const vec4 C = vec4(0.211324865405187, 0.366025403784439, -0.577350269189626, 0.024390243902439);
    vec2 i = floor(v + dot(v, C.yy));
    vec2 x0 = v - i + dot(i, C.xx);
    vec2 i1 = (x0.x > x0.y) ? vec2(1.0, 0.0) : vec2(0.0, 1.0);
    vec4 x12 = x0.xyxy + C.xxzz; x12.xy -= i1;
    i = mod289(i);
    vec3 p = permute(permute(i.y + vec3(0.0, i1.y, 1.0)) + i.x + vec3(0.0, i1.x, 1.0));
    vec3 m = max(0.5 - vec3(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), 0.0);
    m = m * m; m = m * m;
    vec3 x2 = 2.0 * fract(p * C.www) - 1.0;
    vec3 h = abs(x2) - 0.5;
    vec3 ox = floor(x2 + 0.5);
    vec3 a0 = x2 - ox;
    m *= 1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h);
    vec3 g;
    g.x = a0.x * x0.x + h.x * x0.y;
    g.yz = a0.yz * x12.xz + h.yz * x12.yw;
    return 130.0 * dot(m, g);
}
float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }

void main() {
    vec2 uv = vUv;
    float aspect = uRes.x / max(uRes.y, 1.0);
    // 以竖屏为准：x 按宽高比拉伸到与 y 同尺度
    vec2 p = vec2((uv.x - 0.5) * aspect, uv.y);

    float breath = 0.5 + 0.5 * sin(uTime * 0.25);
    float energy = 0.3 + 0.7 * breath + uAudio;
    float horizon = 0.36;

    // ── 夜空渐变 ──
    vec3 skyTop = vec3(0.010, 0.020, 0.050);
    vec3 skyBot = vec3(0.030, 0.090, 0.180);
    float sky = smoothstep(horizon, 1.0, uv.y);
    vec3 col = mix(skyBot, skyTop, sky);

    // ── 星云 ──
    float neb = snoise(vec2(p.x * 2.0 + uTime * 0.012, uv.y * 3.0));
    neb += 0.5 * snoise(vec2(p.x * 4.0 - uTime * 0.008, uv.y * 6.0 + 3.7));
    col += skyBot * neb * 0.10;

    // ── 地平线辉光 ──
    col += vec3(0.05, 0.55, 0.55) * exp(-abs(uv.y - horizon) * 6.0) * (0.30 + 0.25 * energy);

    // ── 星空（地平线之上）──
    if (uv.y > horizon) {
        vec2 sg = floor(vec2(p.x, uv.y) * 150.0);
        float st = hash(sg);
        if (st > 0.992) {
            float tw = 0.5 + 0.5 * sin(uTime * 2.0 + st * 120.0);
            col += vec3(0.75, 0.86, 1.0) * tw * 0.9;
        }
    }

    // ── 极光帘（3 层）──
    for (int k = 0; k < 3; k++) {
        float fk = float(k);
        float base = horizon + 0.13 + fk * 0.10;
        float wave = base
            + sin(p.x * 2.4 + uTime * 0.30 + fk * 1.7) * 0.05
            + snoise(vec2(p.x * 1.5 + fk, uTime * 0.05)) * 0.035;
        float band = exp(-pow((uv.y - wave) * 13.0, 2.0));
        // 竖向流动纹理
        float rays = 0.6 + 0.4 * sin(p.x * 40.0 + snoise(vec2(p.x * 6.0, uTime * 0.1 + fk)) * 5.0);
        vec3 ac = mix(vec3(0.05, 0.85, 0.55), vec3(0.40, 0.20, 0.95), fk / 2.0);
        col += ac * band * rays * (0.30 + 0.25 * energy);
    }

    // ── 地形剪影 + 边缘辉光 ──
    float terr = horizon - 0.05
        + snoise(vec2(p.x * 1.2, 3.7)) * 0.055
        + snoise(vec2(p.x * 3.0 + 1.0, 7.1)) * 0.02;
    float ground = smoothstep(terr + 0.004, terr - 0.004, uv.y); // 地形线以下=1
    vec3 gcol = mix(vec3(0.015, 0.05, 0.09), vec3(0.0, 0.13, 0.16), clamp((terr - uv.y) * 3.0, 0.0, 1.0));
    gcol += vec3(0.10, 0.60, 0.60) * exp(-abs(uv.y - terr) * 42.0) * (0.6 + 0.4 * energy);
    col = mix(col, gcol, ground);

    // ── 暗角 ──
    float vig = smoothstep(1.25, 0.35, length(vec2((uv.x - 0.5) * aspect, uv.y - 0.5)));
    col *= mix(0.72, 1.0, vig);

    gl_FragColor = vec4(col, 1.0);
}
