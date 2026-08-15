import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { commands, type WorktreeHookStatus } from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { useStore } from "@/store";
import { useNav } from "@/store-nav";
import { Button, Modal, ModalBody, ModalFooter, ModalHeader } from "@ryuzi/ui";

const field = "flex h-[34px] items-center rounded-md border border-input bg-background px-3 text-[13px]";

// Ryuzi-only sessions: there is no harness or default-agent choice anymore —
// every project runs the native runtime and models are picked per-composer.
export function ProjectSettingsModal() {
  const projectId = useNav((s) => s.projectSettingsFor);
  const close = useNav((s) => s.setProjectSettingsFor);
  const project = useStore((s) => s.projects.find((p) => p.projectId === projectId));
  if (!projectId || !project) return null;
  return (
    <Modal onClose={() => close(null)} width={460}>
      <ModalHeader
        leading={<FolderOpen aria-hidden className="mt-0.5 size-4 text-muted-foreground" strokeWidth={2} />}
        title="Project settings"
        description={project.name}
      />
      <ModalBody>
        <div className="flex flex-col gap-3.5">
          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-semibold">Name</span>
            <div className={field}>{project.name}</div>
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-semibold">Local path</span>
            <div className={`${field} font-mono text-xs text-muted-foreground`}>{project.workdir}</div>
          </div>
          <HookScripts projectId={projectId} />
        </div>
      </ModalBody>
      <ModalFooter>
        <Button onClick={() => close(null)}>Done</Button>
      </ModalFooter>
    </Modal>
  );
}

// `.ryuzi/hooks/<event>/` executables live in the repository, so anyone who can
// send a pull request can propose code that runs on this machine. The engine
// refuses to execute them until the exact bytes are accepted here; this section
// is the only place that acceptance can be granted.
function HookScripts({ projectId }: { projectId: string }) {
  const [status, setStatus] = useState<WorktreeHookStatus | null>(null);
  useEffect(() => {
    let live = true;
    void commands.worktreeHookStatus(LOCAL_RUNNER, projectId).then((res) => {
      // An RPC failure renders nothing rather than an alarming empty widget —
      // the engine still fails closed, so nothing runs either way.
      if (live && res.status === "ok") setStatus(res.data);
    });
    return () => {
      live = false;
    };
  }, [projectId]);

  if (!status || status.scripts.length === 0) return null;

  const trust = async () => {
    const res = await commands.trustWorktreeHooks(LOCAL_RUNNER, projectId);
    if (res.status === "ok") setStatus(res.data);
  };

  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-xs font-semibold">Hook scripts</span>
      <div className="flex flex-col gap-1 rounded-md border border-input bg-background px-3 py-2">
        {status.scripts.map((script) => (
          <div className="font-mono text-xs text-muted-foreground" key={script}>
            {script}
          </div>
        ))}
      </div>
      {status.trusted ? (
        <span className="text-xs text-muted-foreground">Trusted. Editing any of these scripts will revoke this and stop them running.</span>
      ) : (
        <>
          <span className="text-xs text-muted-foreground">
            These scripts have not been trusted and will not run. Review them before trusting.
          </span>
          <div className="flex">
            <Button onClick={trust}>Trust these scripts</Button>
          </div>
        </>
      )}
    </div>
  );
}
