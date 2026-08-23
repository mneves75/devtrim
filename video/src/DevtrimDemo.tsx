import React from "react";
import {
  AbsoluteFill,
  interpolate,
  spring,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";

const GREEN = "#c8ff38";
const AMBER = "#ffc53d";
const RED = "#ff6b57";
const BG = "#0a0a0a";
const INK = "#f4f1e9";
const MUTED = "#aaa8a1";
const LINE = "rgba(244,241,233,0.14)";

const Mono: React.FC<{children: React.ReactNode}> = ({children}) => (
  <span style={{fontFamily: "var(--mono, monospace)"}}>{children}</span>
);

const TerminalWindow: React.FC<{title: string; children: React.ReactNode; w: number}> = ({
  title,
  children,
  w,
}) => (
  <div
    style={{
      width: w,
      border: `1px solid ${LINE}`,
      borderRadius: 14,
      background: "#111",
      boxShadow: "0 24px 60px rgba(0,0,0,0.5)",
      overflow: "hidden",
    }}
  >
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "10px 14px",
        borderBottom: `1px solid ${LINE}`,
        background: "#161616",
      }}
    >
      <span style={{width: 12, height: 12, borderRadius: "50%", background: "#ff5f57"}} />
      <span style={{width: 12, height: 12, borderRadius: "50%", background: "#febc2e"}} />
      <span style={{width: 12, height: 12, borderRadius: "50%", background: "#28c840"}} />
      <span style={{marginLeft: 10, color: MUTED, fontSize: 18, fontFamily: "monospace"}}>
        {title}
      </span>
    </div>
    <div style={{padding: 22, fontFamily: "monospace", fontSize: 26, lineHeight: 1.7}}>
      {children}
    </div>
  </div>
);

const Line: React.FC<{frame: number; at: number; children: React.ReactNode}> = ({
  frame,
  at,
  children,
}) => {
  const show = frame >= at;
  if (!show) return null;
  return (
    <div style={{whiteSpace: "pre", color: INK}}>{children}</div>
  );
};

export const DevtrimDemo: React.FC = () => {
  const frame = useCurrentFrame();
  const {fps, durationInFrames} = useVideoConfig();
  const pop = (at: number) =>
    spring({frame: frame - at, fps, config: {damping: 200}});

  // end card reveal
  const endOpacity = interpolate(frame, [durationInFrames - 45, durationInFrames - 15], [0, 1], {
    extrapolateLeft: true, extrapolateRight: true,
  });

  return (
    <AbsoluteFill style={{background: BG, padding: 70, justifyContent: "center", gap: 60, flexDirection: "row"}}>
      {/* left: terminal */}
      <TerminalWindow title="zsh — devtrim" w={900}>
        <Line frame={frame} at={5}><span style={{color: GREEN}}>$</span> devtrim scan</Line>
        {[
          {at: 25, size: "43.0 GB", d: 7, label: "DerivedData", note: "rebuilt on next build"},
          {at: 40, size: "21.0 GB", d: 3, label: "huggingface model cache", note: "re-downloads on demand"},
          {at: 55, size: "2.9 GB", d: 5, label: "stale node-modules ×6", note: "repos inactive ≥ 30 days"},
          {at: 70, size: "2.7 GB", d: 4, label: "iOS simulators", note: "17 devices installed"},
        ].map((r) => (
          <Line key={r.label} frame={frame} at={r.at}>
            <div style={{opacity: pop(r.at)}}>
              {" "}
              <span style={{color: AMBER}}>{r.size.padStart(8)}</span>
              {"  danger:"}
              <span style={{color: r.d >= 7 ? RED : r.d >= 5 ? AMBER : GREEN}}>{r.d}</span>
              {"  "}
              {r.label}
              {"\n           └─ "}
              <span style={{color: MUTED}}>{r.note}</span>
            </div>
          </Line>
        ))}
        <Line frame={frame} at={88}>
          <span style={{color: MUTED}}>69.6 GB across 13 item(s)</span>
        </Line>
        <Line frame={frame} at={100}>
          <span style={{color: GREEN}}>$</span> devtrim clean caches --apply -y
        </Line>
        <Line frame={frame} at={115}>
          <span style={{color: GREEN}}>✓ trashed huggingface model cache — recoverable from Finder</span>
        </Line>
        <Line frame={frame} at={125}>
          <span style={{color: GREEN}}>$</span>{" "}
        </Line>
      </TerminalWindow>

      {/* right: verdict panel */}
      <div style={{flex: 1, display: "flex", flexDirection: "column", justifyContent: "center", gap: 34}}>
        {[
          {t: "MEASURE", s: "real bytes on disk", c: INK},
          {t: "CLASSIFY", s: "danger score 1–10", c: AMBER},
          {t: "TRIM", s: "Trash-first · recoverable", c: GREEN},
        ].map((b, i) => (
          <div key={b.t} style={{opacity: pop(130 + i * 12), transform: `translateY(${(1 - pop(130 + i * 12)) * 14}px)`}}>
            <div style={{fontSize: 44, fontWeight: 800, letterSpacing: "-0.02em", color: b.c}}>{b.t}</div>
            <div style={{fontSize: 22, color: MUTED, fontFamily: "monospace"}}>{b.s}</div>
          </div>
        ))}
      </div>

      {/* end card */}
      <AbsoluteFill style={{background: BG, opacity: endOpacity, display: "flex", alignItems: "center", justifyContent: "center", flexDirection: "column", gap: 20}}>
        <div style={{fontSize: 120, fontWeight: 800, letterSpacing: "-0.04em", color: INK}}>
          dev<span style={{color: GREEN}}>trim</span>
        </div>
        <div style={{fontFamily: "monospace", fontSize: 30, color: MUTED}}>github.com/mneves75/devtrim</div>
      </AbsoluteFill>
    </AbsoluteFill>
  );
};
