import { PluginContentsList } from "@/components/plugins/PluginContentsList";
import type { WizardCtx } from "./UniversalInstallWizard";

/** Task 15's "What you get" preview step — planned right after Overview
 *  whenever `ctx.detail` declares any commands/skills/hooks/jobs
 *  (`WizardPlanInput.hasContents`). Reuses the exact same `PluginContentsList`
 *  the plugin detail Contents tab renders (Task 14) so the pre-install
 *  preview and the post-install tab can never show different data for the
 *  same plugin. Hooks/jobs aren't rendered here — unlike the detail view's
 *  Automations tab, there's nothing actionable to do with them before
 *  install (no enable switch, no "Set up…" target to jump to), so this step
 *  sticks to the two lists a user can meaningfully preview before
 *  committing: what commands/skills they're about to get. */
export function ContentsStep({ ctx }: { ctx: WizardCtx; onNext: () => void }) {
  const commands = ctx.detail?.commands ?? [];
  const skills = ctx.detail?.skills ?? [];
  const hookCount = ctx.detail?.hooks.length ?? 0;
  const jobCount = ctx.detail?.jobs.length ?? 0;

  return (
    <div className="flex flex-col gap-3">
      <PluginContentsList commands={commands} skills={skills} />
      {(hookCount > 0 || jobCount > 0) && (
        <p className="m-0 text-[12px] text-muted-foreground">
          Also installs {hookCount > 0 ? `${hookCount} automation ${hookCount === 1 ? "hook" : "hooks"}` : null}
          {hookCount > 0 && jobCount > 0 ? " and " : null}
          {jobCount > 0 ? `${jobCount} scheduled ${jobCount === 1 ? "job" : "jobs"}` : null} — set these up from the plugin's Automations
          tab once installed.
        </p>
      )}
    </div>
  );
}
