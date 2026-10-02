// Siri-style voice wave: a thin glowing line with mirrored lobes that swell with the mic level.
// Usage: const wave = Wave.create(canvas); wave.setMode("listening"); wave.setLevel(0..1)
(function () {
  const PALETTES = {
    listening: ["50,80,255", "70,140,255", "140,190,255", "160,90,255", "215,90,255"],
    thinking: ["110,80,255", "165,100,255", "225,150,255", "165,100,255", "110,80,255"],
    cancel: ["255,150,60", "255,200,110", "255,240,205", "255,170,70", "255,110,60"],
    idle: ["100,120,200", "120,140,220", "150,170,235", "120,140,220", "100,120,200"],
  };
  // amp: relative height, freq: lobes across the width, speed: drift (negative = other way)
  const CURVES = [
    { amp: 1.0, freq: 1.25, speed: 1.9, alpha: 0.2 },
    { amp: 0.74, freq: 1.8, speed: -1.5, alpha: 0.17 },
    { amp: 0.5, freq: 2.5, speed: 2.4, alpha: 0.14 },
    { amp: 0.3, freq: 3.3, speed: -3.0, alpha: 0.11 },
  ];
  const POINTS = 140;

  function create(canvas) {
    const ctx = canvas.getContext("2d");
    let w = 0, h = 0;
    let mode = "idle", level = 0, peak = 0.006, noise = 0.002, amp = 0, t = 0, last = performance.now();

    function fit() {
      const r = canvas.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      w = r.width;
      h = r.height;
      canvas.width = Math.max(1, Math.round(w * dpr));
      canvas.height = Math.max(1, Math.round(h * dpr));
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    }
    fit();
    window.addEventListener("resize", fit);

    // Colour runs left → right; both ends fade to nothing, like the reference.
    function gradient(alpha) {
      const g = ctx.createLinearGradient(0, 0, w, 0);
      const stops = PALETTES[mode] || PALETTES.idle;
      const alphas = [0, 1, 1, 1, 0];
      stops.forEach((rgb, i) => g.addColorStop(i / (stops.length - 1), `rgba(${rgb},${alpha * alphas[i]})`));
      return g;
    }

    function frame(now) {
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      t += dt;
      const target =
        mode === "listening" ? 0.05 + 0.95 * level
        : mode === "thinking" ? 0.5 + 0.2 * Math.sin(t * 3)
        : mode === "cancel" ? 0.42 + 0.15 * Math.sin(t * 8)
        : 0.015;
      amp += (target - amp) * Math.min(1, dt * 10);

      ctx.clearRect(0, 0, w, h);
      ctx.globalCompositeOperation = "lighter";
      const mid = h / 2;
      const maxHeight = h * 0.48;

      // The thin centre line.
      ctx.fillStyle = gradient(0.55);
      ctx.fillRect(0, mid - 0.5, w, 1);

      ctx.shadowBlur = 9;
      ctx.shadowColor = `rgba(${(PALETTES[mode] || PALETTES.idle)[1]},0.55)`;
      for (const c of CURVES) {
        ctx.beginPath();
        const edge = (i) => {
          const x = (i / POINTS) * 2 - 1;
          const envelope = Math.pow(4 / (4 + Math.pow(x * 3, 4)), 2); // tall in the middle, gone at the ends
          return [(i / POINTS) * w, amp * c.amp * envelope * Math.abs(Math.sin(c.freq * x * Math.PI + t * c.speed)) * maxHeight];
        };
        for (let i = 0; i <= POINTS; i++) {
          const [px, a] = edge(i);
          i ? ctx.lineTo(px, mid - a) : ctx.moveTo(px, mid - a);
        }
        for (let i = POINTS; i >= 0; i--) {
          const [px, a] = edge(i);
          ctx.lineTo(px, mid + a);
        }
        ctx.closePath();
        ctx.fillStyle = gradient(c.alpha);
        ctx.fill();
        ctx.strokeStyle = gradient(Math.min(1, c.alpha + 0.3));
        ctx.lineWidth = 1;
        ctx.stroke();
      }
      ctx.shadowBlur = 0;
      requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);

    return {
      getLevel() { return level; },
      setMode(m) { mode = PALETTES[m] ? m : "idle"; },
      // Raw mic RMS. The room's background level is learned (falls fast, rises slowly) so silence stays
      // flat, and speech is scaled between that floor and the recent peak, so even a quiet microphone
      // swings the wave most of the way up.
      setLevel(rms) {
        noise = rms < noise ? rms : noise + (rms - noise) * 0.0005;
        peak = Math.max(peak * 0.996, rms, noise * 4, 0.004);
        const gate = noise * 1.6;
        const x = Math.max(0, (rms - gate) / Math.max(1e-6, peak - gate));
        level = Math.min(1, Math.pow(x, 0.6) * 1.15);
      },
    };
  }

  window.Wave = { create };
})();
