import React from "react";
import {
  AbsoluteFill,
  Audio,
  Easing,
  interpolate,
  staticFile,
  useCurrentFrame,
} from "remotion";

// Landing-page palette (styles.css) and the TUI's named-ANSI roles mapped onto it.
const BG = "#0a0a0a";
const SURFACE = "#111111";
const INK = "#f4f1e9";
const MUTED = "#aaa8a1";
const DIM = "#7c7a74";
const ACCENT = "#c8ff38";
const AMBER = "#ffc53d";
const RED = "#ff6b57";
const CYAN = "#7fd7d0";
const LINE = "rgba(244,241,233,0.22)";
const HAIR = "rgba(244,241,233,0.14)";

const SANS =
  '-apple-system, BlinkMacSystemFont, "SF Pro Display", "Helvetica Neue", sans-serif';
const MONO = 'Menlo, "SF Mono", monospace';

export const DURATION = 645;

const OUT = Easing.bezier(0.16, 1, 0.3, 1);
const INOUT = Easing.bezier(0.65, 0, 0.35, 1);

const ramp = (
  f: number,
  a: number,
  b: number,
  from = 0,
  to = 1,
  easing = OUT,
) =>
  interpolate(f, [a, b], [from, to], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing,
  });

/** Visible from `a` (fading in over `inF`) until `b` (fading out over `outF`). */
const win = (f: number, a: number, b: number, inF = 8, outF = 8) =>
  Math.min(ramp(f, a, a + inF), 1 - ramp(f, b - outF, b, 0, 1, INOUT));

// ---------- terminal model ----------

type Cell = { t: string; c?: string; b?: boolean };
type Row = Cell[];
const COLS = 104;
const FONT = 23;
const LH = 31;

const len = (r: Row) => r.reduce((n, c) => n + c.t.length, 0);
const pad = (r: Row, w: number): Row =>
  len(r) < w ? [...r, { t: " ".repeat(w - len(r)) }] : r;

const box = (title: string, lines: Row[], h: number, w = COLS): Row[] => {
  const top: Row = title
    ? [
        { t: "┌ ", c: LINE },
        { t: title },
        { t: " " + "─".repeat(w - 4 - title.length) + "┐", c: LINE },
      ]
    : [{ t: "┌" + "─".repeat(w - 2) + "┐", c: LINE }];
  const inner: Row[] = [];
  for (let i = 0; i < h; i++)
    inner.push([
      { t: "│", c: LINE },
      ...pad(lines[i] ?? [], w - 2),
      { t: "│", c: LINE },
    ]);
  return [top, ...inner, [{ t: "└" + "─".repeat(w - 2) + "┘", c: LINE }]];
};

// Captured from the real devtrim 0.10.0 TUI (media/brag/source/capture.py); only the
// disposable HOME prefix is shown as /Users/you.
type Finding = {
  size: string;
  label: string;
  path: string;
  note: string;
  project: string;
};
const FINDINGS: Finding[] = [
  {
    size: "8.1 GB",
    label: "stale target artifacts",
    path: "/Users/you/dev/checkout-api/target",
    note: "repo last active 2020-01-01 UTC; corroboration: sibling Cargo.toml",
    project: "/Users/you/dev/checkout-api",
  },
  {
    size: "1.2 GB",
    label: "stale node_modules",
    path: "/Users/you/dev/checkout-api/node_modules",
    note: "repo last active 2020-01-01 UTC; lockfile: none",
    project: "/Users/you/dev/checkout-api",
  },
  {
    size: "2.4 GB",
    label: "stale node_modules",
    path: "/Users/you/dev/marketing-site/node_modules",
    note: "repo last active 2020-01-01 UTC; lockfile: none",
    project: "/Users/you/dev/marketing-site",
  },
  {
    size: "1.8 GB",
    label: "stale .next artifacts",
    path: "/Users/you/dev/marketing-site/.next",
    note: "repo last active 2020-01-01 UTC; corroboration: sibling package.json",
    project: "/Users/you/dev/marketing-site",
  },
  {
    size: "4.1 GB",
    label: "stale .venv artifacts",
    path: "/Users/you/dev/ml-notebooks/.venv",
    note: "repo last active 2020-01-01 UTC; corroboration: contained pyvenv.cfg",
    project: "/Users/you/dev/ml-notebooks",
  },
  {
    size: "1.0 GB",
    label: "stale Pods artifacts",
    path: "/Users/you/dev/ios-client/Pods",
    note: "repo last active 2020-01-01 UTC; corroboration: sibling Podfile",
    project: "/Users/you/dev/ios-client",
  },
];

/** Keeps the end of a path within `max` columns, like the TUI's row clip. */
const clipLeft = (text: string, max: number) =>
  text.length <= max ? text : "…" + text.slice(text.length - max + 1);

const findingRow = (
  i: number,
  cursor: boolean,
  selected: boolean,
): Row => [
  ...findingPrefix(i, cursor, selected),
  {
    t: clipLeft(
      FINDINGS[i].path,
      COLS - 2 - len(findingPrefix(i, cursor, selected)),
    ),
    c: DIM,
  },
];

const findingPrefix = (
  i: number,
  cursor: boolean,
  selected: boolean,
): Row => [
  { t: cursor ? "› " : "  ", c: ACCENT, b: true },
  { t: selected ? "[x]" : "[ ]", c: selected ? INK : DIM },
  { t: "   " },
  { t: `${i + 1}. danger-5`, c: AMBER },
  { t: "   " },
  { t: `${FINDINGS[i].size.padStart(6)}  TRASH`, c: CYAN },
  { t: "    " },
  { t: FINDINGS[i].label, b: true },
  { t: "  " },
];

const CHAR_W = FONT * 0.602;

// Box-drawing glyphs painted like a terminal does: [left, right, up, down] arms.
const ARMS: Record<string, [boolean, boolean, boolean, boolean]> = {
  "─": [true, true, false, false],
  "│": [false, false, true, true],
  "┌": [false, true, false, true],
  "┐": [true, false, false, true],
  "└": [false, true, true, false],
  "┘": [true, false, true, false],
};

const BoxGlyph: React.FC<{ ch: string }> = ({ ch }) => {
  const [l, r, u, d] = ARMS[ch];
  const mid = Math.round(LH / 2);
  const cx = Math.round(CHAR_W / 2);
  const line = { position: "absolute" as const, background: LINE };
  return (
    <span
      style={{
        position: "relative",
        display: "inline-block",
        width: CHAR_W,
        height: LH,
        verticalAlign: "top",
      }}
    >
      {l || r ? (
        <span
          style={{
            ...line,
            top: mid,
            height: 1.5,
            left: l ? 0 : cx,
            right: r ? 0 : CHAR_W - cx,
          }}
        />
      ) : null}
      {u || d ? (
        <span
          style={{
            ...line,
            left: cx,
            width: 1.5,
            top: u ? 0 : mid,
            bottom: d ? 0 : LH - mid,
          }}
        />
      ) : null}
    </span>
  );
};

/** One terminal row on a strict character grid, whatever font fallback does. */
const TermLine: React.FC<{ row: Row; opacity?: number; dx?: number }> = ({
  row,
  opacity = 1,
  dx = 0,
}) => (
  <div
    style={{
      height: LH,
      whiteSpace: "pre",
      opacity,
      transform: `translateX(${dx}px)`,
      display: "flex",
    }}
  >
    {row.flatMap((cell, j) =>
      Array.from(cell.t).map((ch, k) =>
        ARMS[ch] ? (
          <BoxGlyph key={`${j}-${k}`} ch={ch} />
        ) : (
          <span
            key={`${j}-${k}`}
            style={{
              display: "inline-block",
              width: CHAR_W,
              height: LH,
              lineHeight: `${LH}px`,
              textAlign: "center",
              color: cell.c ?? INK,
              fontWeight: cell.b ? 700 : 400,
            }}
          >
            {ch}
          </span>
        ),
      ),
    )}
  </div>
);

// ---------- shared pieces ----------

const Grid: React.FC = () => {
  const f = useCurrentFrame();
  const drift = (f * 0.25) % 64;
  return (
    <AbsoluteFill style={{ backgroundColor: BG }}>
      <AbsoluteFill
        style={{
          backgroundImage: `linear-gradient(${HAIR.replace("0.14", "0.045")} 1px, transparent 1px), linear-gradient(90deg, ${HAIR.replace("0.14", "0.045")} 1px, transparent 1px)`,
          backgroundSize: "64px 64px",
          backgroundPosition: `0 ${drift}px`,
        }}
      />
      <AbsoluteFill
        style={{
          background:
            "radial-gradient(ellipse at 50% 45%, rgba(10,10,10,0) 35%, rgba(10,10,10,0.92) 100%)",
        }}
      />
    </AbsoluteFill>
  );
};

const Keycap: React.FC<{ label: string; at: number; until: number }> = ({
  label,
  at,
  until,
}) => {
  const f = useCurrentFrame();
  const o = win(f, at - 6, until, 6, 6);
  const press = f >= at && f < at + 4 ? 4 : 0;
  const pop = ramp(f, at - 6, at, 0.85, 1);
  return (
    <div
      style={{
        opacity: o,
        transform: `translateY(${press}px) scale(${pop})`,
        minWidth: 88,
        height: 76,
        padding: "0 26px",
        borderRadius: 14,
        border: `1px solid ${f >= at ? ACCENT : "rgba(244,241,233,0.3)"}`,
        background: "#161616",
        boxShadow: `0 ${6 - press}px 0 #050505`,
        color: f >= at ? ACCENT : INK,
        fontFamily: MONO,
        fontSize: 32,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      {label}
    </div>
  );
};

// ---------- scene 1: disk meter ----------

const HookScene: React.FC = () => {
  const f = useCurrentFrame();
  if (f > 80) return null;
  const pct = Math.round(ramp(f, 4, 40, 61, 97));
  const hot = ramp(f, 26, 40);
  const color = pct >= 97 ? RED : pct >= 85 ? AMBER : INK;
  const exit = ramp(f, 60, 73, 0, 1, INOUT);
  const lock = f >= 40 && f < 46 ? 1.03 : 1;
  const barW = 1180;
  const fillW = (barW * pct) / 100;
  return (
    <AbsoluteFill
      style={{
        alignItems: "center",
        justifyContent: "center",
        fontFamily: SANS,
      }}
    >
      <div
        style={{
          opacity: 1 - exit,
          transform: `translateY(${-30 * exit}px) scale(${lock})`,
          display: "flex",
          alignItems: "baseline",
          gap: 20,
          marginBottom: 36,
        }}
      >
        <div
          style={{
            fontSize: 250,
            fontWeight: 800,
            letterSpacing: "-0.05em",
            color,
            fontVariantNumeric: "tabular-nums",
            lineHeight: 1,
          }}
        >
          {pct}%
        </div>
        <div style={{ fontSize: 56, fontWeight: 700, color: MUTED }}>used</div>
      </div>
      <div
        style={{
          width: barW * (1 - exit * 0.96),
          opacity: 1 - exit,
          height: 30,
          borderRadius: 6,
          background: "rgba(244,241,233,0.07)",
          border: `1px solid ${HAIR}`,
          position: "relative",
          overflow: "hidden",
        }}
      >
        <div
          style={{
            position: "absolute",
            inset: 0,
            width: fillW * (1 - exit * 0.96),
            background: `linear-gradient(90deg, ${AMBER} 0%, ${hot > 0.5 ? RED : AMBER} 100%)`,
            opacity: 0.92,
          }}
        />
        {Array.from({ length: 9 }, (_, i) => (
          <div
            key={i}
            style={{
              position: "absolute",
              left: `${(i + 1) * 10}%`,
              top: 0,
              bottom: 0,
              width: 2,
              background: "rgba(10,10,10,0.55)",
              opacity: 1 - exit,
            }}
          />
        ))}
      </div>
      <div
        style={{
          marginTop: 40,
          fontSize: 52,
          fontWeight: 700,
          letterSpacing: "-0.02em",
          color: INK,
          opacity: win(f, 20, 72, 8, 10),
          transform: `translateY(${ramp(f, 20, 30, 16, 0)}px)`,
        }}
      >
        Disk almost full. <span style={{ color: MUTED }}>Again.</span>
      </div>
    </AbsoluteFill>
  );
};

// ---------- scene 2 + 6: wordmark ----------

const Wordmark: React.FC<{ start: number; size: number }> = ({
  start,
  size,
}) => {
  const f = useCurrentFrame();
  const reveal = ramp(f, start, start + 18);
  const blink = Math.floor((f - start) / 15) % 2 === 0 || f < start + 24;
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        fontFamily: SANS,
        fontSize: size,
        fontWeight: 800,
        letterSpacing: "-0.055em",
        color: INK,
        lineHeight: 1,
      }}
    >
      <span
        style={{
          clipPath: `inset(-20% ${100 - reveal * 100}% -20% 0)`,
          paddingRight: size * 0.06,
        }}
      >
        devtrim
      </span>
      <span
        style={{
          width: size * 0.42,
          height: size * 0.78,
          background: ACCENT,
          marginLeft: size * 0.04,
          opacity: (blink ? 1 : 0.15) * ramp(f, start + 2, start + 10),
          transform: `translateY(${size * 0.04}px)`,
        }}
      />
    </div>
  );
};

const RevealScene: React.FC = () => {
  const f = useCurrentFrame();
  if (f < 74 || f > 170) return null;
  const exit = ramp(f, 154, 166, 0, 1, INOUT);
  const words = ["Measure.", "Classify.", "Trim.", "Safely."];
  return (
    <AbsoluteFill
      style={{
        alignItems: "center",
        justifyContent: "center",
        fontFamily: SANS,
        opacity: 1 - exit,
        transform: `scale(${1 - 0.04 * exit})`,
      }}
    >
      <div
        style={{
          fontSize: 26,
          fontWeight: 700,
          letterSpacing: "0.16em",
          textTransform: "uppercase",
          color: DIM,
          marginBottom: 34,
          opacity: ramp(f, 82, 94),
        }}
      >
        Disk hygiene for macOS developer machines
      </div>
      <Wordmark start={76} size={230} />
      <div
        style={{
          display: "flex",
          gap: 22,
          marginTop: 46,
          fontSize: 58,
          fontWeight: 700,
          letterSpacing: "-0.025em",
        }}
      >
        {words.map((w, i) => {
          const a = 96 + i * 4;
          return (
            <span
              key={w}
              style={{
                color: i === 3 ? ACCENT : INK,
                opacity: ramp(f, a, a + 8),
                transform: `translateY(${ramp(f, a, a + 10, 18, 0)}px)`,
              }}
            >
              {w}
            </span>
          );
        })}
      </div>
    </AbsoluteFill>
  );
};

// ---------- scene 3 + 4: the real TUI ----------

const T_IN = 166;
const T_ROWS = 188;
const K_J = 264;
const K_SPACE = 286;
const MODAL = 348;
const T_OUT = 452;

const Caption: React.FC<{
  a: number;
  b: number;
  children: React.ReactNode;
  sub?: React.ReactNode;
}> = ({ a, b, children, sub }) => {
  const f = useCurrentFrame();
  const o = win(f, a, b, 9, 7);
  if (o <= 0) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: 0,
        top: 0,
        opacity: o,
        transform: `translateY(${ramp(f, a, a + 12, 14, 0)}px)`,
      }}
    >
      <div
        style={{
          fontSize: 50,
          fontWeight: 750,
          letterSpacing: "-0.025em",
          color: INK,
          lineHeight: 1.1,
        }}
      >
        {children}
      </div>
      {sub ? (
        <div style={{ fontSize: 30, color: MUTED, marginTop: 12 }}>{sub}</div>
      ) : null}
    </div>
  );
};

const Code: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <span
    style={{
      fontFamily: MONO,
      fontSize: "0.86em",
      color: ACCENT,
      background: "rgba(200,255,56,0.09)",
      padding: "0.02em 0.22em",
      borderRadius: 8,
    }}
  >
    {children}
  </span>
);

const TuiScene: React.FC = () => {
  const f = useCurrentFrame();
  if (f < T_IN - 2 || f > T_OUT + 12) return null;

  const cursor = f >= K_J ? 1 : 0;
  const leftOut = f >= K_SPACE;
  const title = leftOut
    ? "TRASH-FIRST · 5/6 selected · 17.4 GB · danger-7"
    : "TRASH-FIRST · 6/6 selected · 18.6 GB · danger-7";
  const modal = f >= MODAL;

  const header = box(
    "measure · classify · trim",
    [
      [
        { t: " " },
        { t: "devtrim", c: ACCENT, b: true },
        { t: " " },
        { t: "v0.10.0", c: DIM },
        { t: "  purge" },
      ],
    ],
    1,
  );
  const list = box(
    title,
    FINDINGS.map((_, i) => findingRow(i, i === cursor, !(leftOut && i === 1))),
    6,
  );
  const fd = FINDINGS[cursor];
  const detail: Row[] = [
    [{ t: fd.label, c: ACCENT, b: true }],
    [{ t: `${fd.size} · danger-5 · move to Trash` }],
    [{ t: fd.path }],
    [{ t: fd.note, c: DIM }],
    [{ t: `project: ${fd.project}` }],
  ];
  if (leftOut && cursor === 1)
    detail.push([
      { t: "Left out of this plan; Space adds it back.", c: AMBER },
    ]);
  const details = box("Details", detail, 6);
  const footer = box(
    "",
    modal
      ? [
          [
            {
              t: "Esc cancel · type the exact requested acknowledgment",
              c: CYAN,
            },
          ],
          [],
        ]
      : [
          [
            {
              t: "Space select · A all · a apply · s permanent · b back · ? keys",
              c: CYAN,
            },
          ],
          [
            {
              t: "Review every finding. Space leaves one out; a applies the rest.",
              c: AMBER,
            },
          ],
        ],
    2,
  );

  const enter = ramp(f, T_IN, T_IN + 20);
  const exit = ramp(f, T_OUT - 10, T_OUT + 4, 0, 1, INOUT);
  const titleFlash = leftOut ? 1 - ramp(f, K_SPACE, K_SPACE + 18) : 0;
  const charW = FONT * 0.602;
  const termW = Math.ceil(COLS * charW) + 56;
  const left = (1920 - termW) / 2;

  const rowsIn = (i: number) => ramp(f, T_ROWS + i * 5, T_ROWS + i * 5 + 10);

  const modalLines: Row[] = [
    [{ t: "DATA-LOSS WARNING", c: RED, b: true }],
    [{ t: "This plan: 5 of 6 selected · 1 left out" }],
    [
      {
        t: "Applying this plan can delete data. devtrim is provided AS IS, without warranties; you assume the",
      },
    ],
    [
      {
        t: "risk for the exact targets shown. Keep backups and grant macOS permissions manually only when you",
      },
    ],
    [{ t: "understand the request." }],
    [],
    [
      {
        t: "Danger-7. Press y to apply this exact plan, or n/Esc to cancel.",
        c: AMBER,
      },
    ],
    [],
    [{ t: ">", c: ACCENT }],
  ];
  const modalBox = box("Confirm exact plan", modalLines, 9, 100);
  const mIn = ramp(f, MODAL, MODAL + 10);

  return (
    <AbsoluteFill style={{ fontFamily: SANS }}>
      <div style={{ position: "absolute", left, top: 64, width: termW }}>
        <Caption
          a={T_IN + 8}
          b={K_J - 4}
          sub="One plan, grouped by project, biggest first."
        >
          Every stale <Code>node_modules</Code> and build folder.
        </Caption>
        <Caption a={K_J + 2} b={MODAL - 2}>
          <span style={{ color: ACCENT }}>Space</span> leaves anything out.
        </Caption>
        <Caption a={MODAL + 4} b={T_OUT - 6} sub="Trash first. Recoverable until you empty it.">
          Nothing moves until you approve the exact plan.
        </Caption>
        <div
          style={{
            position: "absolute",
            right: 0,
            top: 4,
            display: "flex",
            gap: 14,
          }}
        >
          <Keycap label="j" at={K_J} until={K_SPACE - 4} />
          <Keycap label="Space" at={K_SPACE} until={MODAL - 6} />
          <Keycap label="a" at={MODAL - 2} until={MODAL + 22} />
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          left,
          top: 196,
          width: termW,
          borderRadius: 16,
          border: `1px solid ${HAIR}`,
          background: SURFACE,
          boxShadow: "0 30px 90px rgba(0,0,0,0.6)",
          overflow: "hidden",
          opacity: enter * (1 - exit),
          transform: `translateY(${(1 - enter) * 70 - exit * 30}px) scale(${1 - exit * 0.03})`,
        }}
      >
        <div
          style={{
            height: 42,
            borderBottom: `1px solid ${HAIR}`,
            display: "flex",
            alignItems: "center",
            padding: "0 18px",
            gap: 9,
          }}
        >
          {[0, 1, 2].map((i) => (
            <div
              key={i}
              style={{
                width: 13,
                height: 13,
                borderRadius: 7,
                background: "rgba(244,241,233,0.18)",
              }}
            />
          ))}
          <div
            style={{
              flex: 1,
              textAlign: "center",
              color: DIM,
              fontSize: 19,
              fontFamily: SANS,
              marginRight: 57,
            }}
          >
            devtrim — 104×23
          </div>
        </div>
        <div
          style={{
            padding: "18px 28px",
            fontFamily: MONO,
            fontSize: FONT,
            background: "#0b0b0b",
            position: "relative",
          }}
        >
          <div style={{ opacity: modal ? 1 - 0.55 * mIn : 1 }}>
            {header.map((r, i) => (
              <TermLine key={`h${i}`} row={r} />
            ))}
            <TermLine
              row={list[0]}
              opacity={1}
              key="lt"
            />
            <div
              style={{
                position: "absolute",
                left: 28 + charW * 2,
                top: 18 + LH * 3,
                height: LH,
                width: charW * (title.length + 2),
                background: `rgba(200,255,56,${0.22 * titleFlash})`,
              }}
            />
            {list.slice(1, 7).map((r, i) => (
              <TermLine
                key={`l${i}`}
                row={r}
                opacity={rowsIn(i)}
                dx={(1 - rowsIn(i)) * -24}
              />
            ))}
            {list.slice(7).map((r, i) => (
              <TermLine key={`lb${i}`} row={r} />
            ))}
            {details.map((r, i) => (
              <TermLine key={`d${i}`} row={r} opacity={i === 0 || i === details.length - 1 ? 1 : ramp(f, T_ROWS + 30, T_ROWS + 42)} />
            ))}
            {footer.map((r, i) => (
              <TermLine key={`f${i}`} row={r} />
            ))}
          </div>
          {modal ? (
            <div
              style={{
                position: "absolute",
                left: 28 + charW * 2,
                top: 18 + LH * 6,
                background: "#0b0b0b",
                opacity: mIn,
                transform: `scale(${0.97 + 0.03 * mIn})`,
                boxShadow: "0 20px 60px rgba(0,0,0,0.7)",
              }}
            >
              {modalBox.map((r, i) => (
                <TermLine key={`m${i}`} row={r} />
              ))}
            </div>
          ) : null}
        </div>
      </div>
    </AbsoluteFill>
  );
};

// ---------- scene 5: the report ----------

const STAT_IN = 458;
const ROLL = 478;
const STAT_OUT = 540;

const StatScene: React.FC = () => {
  const f = useCurrentFrame();
  if (f < STAT_IN - 2 || f > STAT_OUT + 4) return null;
  const p = ramp(f, ROLL, ROLL + 26, 0, 1, INOUT);
  const n = Math.round(1660 - p * (1660 - 53));
  const o = win(f, STAT_IN, STAT_OUT, 10, 10);
  const barW = 1200;
  return (
    <AbsoluteFill
      style={{
        alignItems: "center",
        justifyContent: "center",
        fontFamily: SANS,
        opacity: o,
      }}
    >
      <div
        style={{
          fontSize: 30,
          color: MUTED,
          marginBottom: 18,
          opacity: ramp(f, STAT_IN, STAT_IN + 10),
        }}
      >
        <span style={{ fontFamily: MONO, color: INK }}>devtrim scan</span> on
        the Mac that was 97% full
      </div>
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          gap: 26,
          transform: `translateY(${ramp(f, STAT_IN, STAT_IN + 14, 20, 0)}px)`,
        }}
      >
        <div
          style={{
            fontSize: 270,
            fontWeight: 800,
            letterSpacing: "-0.05em",
            lineHeight: 1,
            fontVariantNumeric: "tabular-nums",
            color: p >= 1 ? ACCENT : INK,
          }}
        >
          {n.toLocaleString("en-US")}
        </div>
        <div style={{ fontSize: 64, fontWeight: 700, color: MUTED }}>
          lines
        </div>
      </div>
      <div
        style={{
          width: barW,
          height: 14,
          marginTop: 30,
          borderRadius: 4,
          background: "rgba(244,241,233,0.07)",
          position: "relative",
        }}
      >
        <div
          style={{
            position: "absolute",
            left: 0,
            top: 0,
            bottom: 0,
            borderRadius: 4,
            width: barW * (n / 1660),
            background: p >= 1 ? ACCENT : INK,
            opacity: 0.85,
          }}
        />
      </div>
      <div
        style={{
          marginTop: 44,
          fontSize: 50,
          fontWeight: 750,
          letterSpacing: "-0.025em",
          color: INK,
          opacity: ramp(f, ROLL + 20, ROLL + 30),
          transform: `translateY(${ramp(f, ROLL + 20, ROLL + 32, 14, 0)}px)`,
        }}
      >
        One line per category, largest first.
      </div>
    </AbsoluteFill>
  );
};

// ---------- scene 6: outro ----------

const OUTRO = 544;
const CMD = "brew install mneves75/devtrim/devtrim";

const OutroScene: React.FC = () => {
  const f = useCurrentFrame();
  if (f < OUTRO - 2) return null;
  const typed = Math.floor(ramp(f, OUTRO + 14, OUTRO + 38, 0, CMD.length, (x) => x));
  const chipIn = ramp(f, OUTRO + 8, OUTRO + 18);
  return (
    <AbsoluteFill
      style={{
        alignItems: "center",
        justifyContent: "center",
        fontFamily: SANS,
      }}
    >
      <div style={{ opacity: ramp(f, OUTRO, OUTRO + 8) }}>
        <Wordmark start={OUTRO} size={170} />
      </div>
      <div
        style={{
          marginTop: 56,
          padding: "22px 34px",
          borderRadius: 14,
          border: `1px solid ${HAIR}`,
          background: "#141414",
          fontFamily: MONO,
          fontSize: 38,
          color: INK,
          whiteSpace: "pre",
          opacity: chipIn,
          transform: `translateY(${(1 - chipIn) * 16}px)`,
        }}
      >
        <span style={{ color: DIM }}>$ </span>
        {CMD.slice(0, typed)}
        <span
          style={{
            color: ACCENT,
            opacity: typed < CMD.length || Math.floor(f / 15) % 2 === 0 ? 1 : 0,
          }}
        >
          ▍
        </span>
        <span style={{ color: "transparent" }}>{CMD.slice(typed)}</span>
      </div>
      <div
        style={{
          marginTop: 40,
          fontSize: 30,
          color: MUTED,
          opacity: ramp(f, OUTRO + 30, OUTRO + 40),
        }}
      >
        Free and open source · Apache-2.0 · macOS ·{" "}
        <span style={{ color: INK }}>github.com/mneves75/devtrim</span>
      </div>
    </AbsoluteFill>
  );
};

export const Brag: React.FC = () => (
  <AbsoluteFill style={{ backgroundColor: BG }}>
    <Grid />
    <HookScene />
    <RevealScene />
    <TuiScene />
    <StatScene />
    <OutroScene />
    <Audio src={staticFile("mix.wav")} />
  </AbsoluteFill>
);
