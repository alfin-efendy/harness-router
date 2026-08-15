import { Button, Modal, ModalBody, ModalFooter, ModalHeader } from "@ryuzi/ui";
import type { AgentImportResultInfo } from "@/bindings";
import { useNav } from "@/store-nav";

/**
 * Reports what an agent-bundle import landed. Unresolvable references are not
 * an import failure: the agent is committed and flagged for repair through the
 * SAME validation surface an on-disk-invalid profile uses (the Invalid badge
 * plus the detail page's "Configuration issues" card), so this modal only has
 * to name them and offer a way over there.
 */
export function AgentImportModal({ result, onClose }: { result: AgentImportResultInfo | null; onClose: () => void }) {
  const nav = useNav();
  if (!result) return null;

  const open = () => {
    nav.navigate({ kind: "agentDetail", agentId: result.agentId });
    onClose();
  };

  return (
    <Modal onClose={onClose} width={460}>
      <ModalHeader title={`Imported ${result.agentName}`} />
      <ModalBody>
        <div className="flex flex-col gap-2 text-[13px] leading-5 text-muted-foreground">
          <p className="m-0">
            {result.knowledgeFilesWritten} knowledge {result.knowledgeFilesWritten === 1 ? "file" : "files"} imported.
          </p>
          {result.renamed && <p className="m-0">Renamed to avoid a name collision.</p>}
          {result.tolerated.length > 0 ? (
            <>
              <p className="m-0">This agent is not executable yet — these references do not exist on this machine:</p>
              <ul className="mb-0 mt-0 pl-4 text-xs text-destructive">
                {result.tolerated.map((issue) => (
                  <li key={`${issue.field}:${issue.message}`}>
                    <strong>{issue.field}:</strong> {issue.message}
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <p className="m-0">Ready to use.</p>
          )}
        </div>
      </ModalBody>
      <ModalFooter>
        <Button variant="outline" onClick={onClose}>
          Close
        </Button>
        <Button onClick={open}>Open agent</Button>
      </ModalFooter>
    </Modal>
  );
}
