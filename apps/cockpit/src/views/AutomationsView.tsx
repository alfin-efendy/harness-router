import { useState } from "react";
import { Segmented } from "@ryuzi/ui";
import { CommandsTab } from "./CommandsTab";
import { HooksTab } from "./HooksTab";
import { SchedulerView } from "./SchedulerView";

type AutomationTab = "scheduler" | "hooks" | "commands";

const TABS: { id: AutomationTab; label: string }[] = [
  { id: "scheduler", label: "Scheduler" },
  { id: "hooks", label: "Hooks" },
  { id: "commands", label: "Commands" },
];

export function AutomationsView({
  initialTab = "scheduler",
  targetId,
}: {
  initialTab?: AutomationTab;
  /** Task 14's plugin detail Automations tab "Set up…" deep link
   *  (`View`'s `automations.targetId`) — forwarded to whichever tab it
   *  targets so that hook/job's editor opens once loaded (Task 16). */
  targetId?: string;
}) {
  const [tab, setTab] = useState<AutomationTab>(initialTab);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 border-b border-border px-8 py-3">
        <div className="mx-auto max-w-[860px]">
          <Segmented options={TABS} value={tab} onChange={setTab} />
        </div>
      </div>
      {tab === "scheduler" ? (
        <SchedulerView targetJobId={targetId} />
      ) : tab === "commands" ? (
        <CommandsTab />
      ) : (
        <HooksTab targetHookId={targetId} />
      )}
    </div>
  );
}
