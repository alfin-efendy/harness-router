import { type CSSProperties, useEffect, useState } from "react";
import { commands } from "@/bindings";
import { PET_COLS, PET_FRAME_H, PET_FRAME_W, PET_ROWS, POSE_ROW, type PetPose } from "@/lib/pet-sprite";

export interface PetSpriteProps {
  slug: string;
  /** true -> bundled asset at `/pets/${slug}/sprite.webp`; false -> fetched via `commands.getPetSprite` and rendered as a data URL. */
  bundled: boolean;
  /** Rendered box width (px); height follows the sheet's frame aspect ratio. */
  size: number;
  pose?: PetPose;
  /** Default true. false, or the OS's reduced-motion preference, freezes on the pose row's first frame. */
  animate?: boolean;
  /** Rendered as the existing color-tile swatch while the sprite is loading or unavailable. */
  fallbackColor: string;
}

// Non-bundled sprites are fetched once per slug over IPC and re-encoded as a
// data URL; every other <PetSprite> instance for that slug (across mounts,
// re-renders, and the whole app session) reuses the cached URL instead of
// re-issuing the command.
const dataUrlCache = new Map<string, string>();

async function resolveSrc(slug: string, bundled: boolean): Promise<string | null> {
  if (bundled) return `/pets/${slug}/sprite.webp`;
  const cached = dataUrlCache.get(slug);
  if (cached !== undefined) return cached;
  try {
    const result = await commands.getPetSprite(slug);
    if (result.status !== "ok" || result.data === null) return null;
    const dataUrl = `data:image/webp;base64,${result.data}`;
    dataUrlCache.set(slug, dataUrl);
    return dataUrl;
  } catch {
    return null;
  }
}

// happy-dom/jsdom don't always implement matchMedia — treat a missing (or
// throwing) matchMedia as "no preference" rather than crash. Checked fresh
// per render (like the reduced-motion check in Transcript.tsx) rather than
// wired up to a change listener, since pose/status changes already keep
// this component re-rendering.
function prefersReducedMotion(): boolean {
  try {
    return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  } catch {
    return false;
  }
}

function FallbackTile({ size, height, color }: { size: number; height: number; color: string }) {
  return (
    <span
      aria-hidden
      data-testid="pet-sprite-fallback"
      className="shrink-0 rounded-lg border border-white/10"
      style={{ width: size, height, backgroundColor: color }}
    />
  );
}

export function PetSprite({ slug, bundled, size, pose = "idle", animate = true, fallbackColor }: PetSpriteProps) {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    resolveSrc(slug, bundled).then((resolved) => {
      if (!cancelled) setSrc(resolved);
    });
    return () => {
      cancelled = true;
    };
  }, [slug, bundled]);

  const scale = size / PET_FRAME_W;
  const frameHeight = PET_FRAME_H * scale;

  if (src === null) return <FallbackTile size={size} height={frameHeight} color={fallbackColor} />;

  const shouldAnimate = animate && !prefersReducedMotion();
  const backgroundWidth = PET_COLS * size;
  const backgroundHeight = PET_ROWS * frameHeight;
  const style: CSSProperties = {
    width: size,
    height: frameHeight,
    backgroundImage: `url(${src})`,
    backgroundSize: `${backgroundWidth}px ${backgroundHeight}px`,
    backgroundPositionX: 0,
    backgroundPositionY: -(POSE_ROW[pose] * frameHeight),
    imageRendering: "pixelated",
  };
  if (shouldAnimate) {
    style.animation = `pet-sprite-cycle 1s steps(${PET_COLS}) infinite`;
    (style as Record<string, string>)["--pet-sprite-total-w"] = `${backgroundWidth}px`;
  }

  return <div aria-hidden data-testid="pet-sprite" className="shrink-0 rounded-lg" style={style} />;
}
