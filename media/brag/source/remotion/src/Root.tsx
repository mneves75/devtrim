import React from "react";
import { Composition } from "remotion";
import { Brag, DURATION } from "./Brag";

export const RemotionRoot: React.FC = () => (
  <Composition
    id="Brag"
    component={Brag}
    durationInFrames={DURATION}
    fps={30}
    width={1920}
    height={1080}
  />
);
