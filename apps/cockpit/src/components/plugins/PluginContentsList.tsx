import {
  SettingsCard as Card,
  SettingsCardHeader as CardHeader,
  SettingsCardRow as CardRow,
  SettingsCardTitle as CardTitle,
} from "@ryuzi/ui";

/**
 * Shared Contents rendering (Task 14's plugin detail Contents tab, Task 15's
 * wizard "What you get" step) — two `CardRow` lists straight off
 * `PluginDetail`: commands as `/name` monospace, skills as their directory
 * name. `PluginDetail.commands`/`.skills` are name-only string arrays (no
 * per-item description — Task 12 never added one), so that's all there is
 * to show; renders nothing (not even an empty Card shell) when both are
 * empty, matching every other conditionally-rendered card in these views.
 */
export function PluginContentsList({ commands, skills }: { commands: string[]; skills: string[] }) {
  if (commands.length === 0 && skills.length === 0) return null;
  return (
    <>
      {commands.length > 0 && (
        <Card className="mb-3">
          <CardHeader>
            <CardTitle>Commands</CardTitle>
          </CardHeader>
          {commands.map((name) => (
            <CardRow key={name}>
              <span className="min-w-0 flex-1 truncate font-mono text-xs">/{name}</span>
            </CardRow>
          ))}
        </Card>
      )}
      {skills.length > 0 && (
        <Card className="mb-3">
          <CardHeader>
            <CardTitle>Skills</CardTitle>
          </CardHeader>
          {skills.map((name) => (
            <CardRow key={name}>
              <span className="min-w-0 flex-1 truncate text-[13px]">{name}</span>
            </CardRow>
          ))}
        </Card>
      )}
    </>
  );
}
