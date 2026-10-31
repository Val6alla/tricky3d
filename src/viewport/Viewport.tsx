import { lazy, Suspense, useEffect, useRef } from "react";
import { CircleCheck, CircleX } from "lucide-react";
import { collisionStatus } from "../analysis/CollisionPanel";
import { useCollisions } from "../analysis/collisions";
import { useLibrary } from "../library/store";
import { useUi } from "../ui";
import { registerViewport } from "./api";
import { createViewportEngine } from "./engine";
import "./viewport.css";

// The Part library's gallery loads when the library first opens.
const LibraryBrowser = lazy(() => import("../library/LibraryBrowser").then((m) => ({ default: m.LibraryBrowser })));

/** Collision detection's status over the view while its panel is open. */
function CollisionBadge() {
  const open = useCollisions((c) => c.open);
  const result = useCollisions((c) => c.result);
  const running = useCollisions((c) => c.running);
  if (!open) return null;
  const clear = !result || result.pairs.length === 0;
  return (
    <div className={`viewport-collisions ${clear ? "is-clear" : "is-hit"}`} role="status">
      {clear ? <CircleCheck size={14} aria-hidden="true" /> : <CircleX size={14} aria-hidden="true" />}
      {running !== null && !result ? "Checking for collisions…" : collisionStatus(result?.pairs ?? [])}
    </div>
  );
}

export function Viewport() {
  const hostRef = useRef<HTMLDivElement>(null);
  const cubeRef = useRef<HTMLDivElement>(null);
  const dropActive = useUi((s) => s.dropActive);
  const libraryOpen = useLibrary((s) => s.session !== null);

  useEffect(() => {
    const host = hostRef.current;
    const cube = cubeRef.current;
    if (!host || !cube) return;
    const engine = createViewportEngine(host, cube);
    registerViewport(engine.api);
    return () => {
      registerViewport(null);
      engine.dispose();
    };
  }, []);

  return (
    <div className="viewport" ref={hostRef}>
      <div className="viewport-cube" ref={cubeRef} />
      {libraryOpen && (
        <Suspense fallback={null}>
          <LibraryBrowser />
        </Suspense>
      )}
      <CollisionBadge />
      {dropActive && (
        <div className="viewport-drop" aria-hidden="true">
          Drop mesh files to import
        </div>
      )}
    </div>
  );
}
