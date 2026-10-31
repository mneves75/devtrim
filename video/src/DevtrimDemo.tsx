import React from "react";
import { AbsoluteFill, Sequence } from "remotion";
import { Confirm } from "./film/scenes/Confirm";
import { Details } from "./film/scenes/Details";
import { Install } from "./film/scenes/Install";
import { Preview } from "./film/scenes/Preview";
import { Select } from "./film/scenes/Select";
import { Title } from "./film/scenes/Title";
import { Trash } from "./film/scenes/Trash";
import { Approve, See } from "./film/scenes/TypeCard";
import { C, timing } from "./film/theme";

const SCENES: Record<string, React.FC> = {
  title: Title,
  preview: Preview,
  see: See,
  details: Details,
  select: Select,
  approve: Approve,
  confirm: Confirm,
  trash: Trash,
  install: Install,
};

/** The demo film: every scene's start and length come from film/timing.json. */
export const DevtrimDemo: React.FC = () => (
  <AbsoluteFill style={{ backgroundColor: C.bg }}>
    {timing.scenes.map((scene) => {
      const Scene = SCENES[scene.id];
      if (!Scene) throw new Error(`no component for scene ${scene.id}`);
      return (
        <Sequence
          key={scene.id}
          from={scene.from}
          durationInFrames={scene.to - scene.from}
          name={scene.id}
        >
          <Scene />
        </Sequence>
      );
    })}
  </AbsoluteFill>
);
