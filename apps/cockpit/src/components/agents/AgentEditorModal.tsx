import { useEffect, useRef, useState } from "react";
import { Button, FormField, Input, Modal, ModalBody, ModalFooter, ModalHeader, Textarea } from "@ryuzi/ui";
import type { AgentMutationInfo, AgentRegistryInfo } from "@/bindings";
import { useBundledPets } from "@/lib/bundled-pets";
import { FRESH_AGENT_PET } from "@/lib/pet-sprite";
import { useAgents } from "@/store-agents";
import { useNav } from "@/store-nav";
import { AgentAvatar } from "./AgentAvatar";
import { PetPicker } from "./PetPicker";

function initialMutation(registry: AgentRegistryInfo): AgentMutationInfo {
  return {
    name: "",
    description: "",
    // Inert back-compat value — the backend requires a string here but the
    // UI no longer renders avatar colors anywhere (pets ARE the avatar).
    avatarColor: "violet",
    avatarPet: null,
    model: registry.subagentModel,
    personality: { preset: "helpful", custom: null },
    permissionRules: [],
    skills: [],
    nativeTools: [],
    pluginTools: [],
    apps: [],
  };
}

export function AgentEditorModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const registry = useAgents((s) => s.registry);
  const saving = useAgents((s) => s.saving);
  const [draft, setDraft] = useState<AgentMutationInfo | null>(() => (registry ? initialMutation(registry) : null));
  const [petPickerOpen, setPetPickerOpen] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const nav = useNav();

  useEffect(() => {
    if (open && registry) setDraft(initialMutation(registry));
  }, [open, registry]);

  const bundledPets = useBundledPets();
  // Prefill a random bundled avatar (never sprout — the Fresh Agent's
  // reserved identity) once the roster lands, only while the draft still
  // has none. Re-runs after each open-reset; it never fights a user pick,
  // since a chosen avatar can only be changed, not cleared.
  useEffect(() => {
    if (!open) return;
    const candidates = bundledPets.filter((pet) => pet.slug !== FRESH_AGENT_PET);
    if (candidates.length === 0) return;
    setDraft((current) =>
      current && current.avatarPet === null
        ? { ...current, avatarPet: candidates[Math.floor(Math.random() * candidates.length)].slug }
        : current,
    );
  }, [open, bundledPets]);

  if (!open || !draft) return null;
  const valid = draft.name.trim().length > 0 && draft.description.trim().length > 0;

  const create = async () => {
    if (!valid || saving) return;
    const created = await useAgents.getState().create({
      ...draft,
      name: draft.name.trim(),
      description: draft.description.trim(),
    });
    if (!created) return;
    onClose();
    nav.navigate({ kind: "agentDetail", agentId: created.summary.id });
  };

  return (
    <Modal onClose={onClose} width={480} busy={saving} initialFocus={nameRef}>
      <ModalHeader title="New agent" description="Create a persistent main agent with isolated configuration and knowledge." />
      <ModalBody className="flex flex-col gap-4">
        <FormField label="Name">
          <Input
            ref={nameRef}
            value={draft.name}
            onChange={(event) => setDraft((current) => (current ? { ...current, name: event.target.value } : current))}
            placeholder="Reviewer"
          />
        </FormField>
        <FormField label="Description" hint="Explain the agent's role and operating focus.">
          <Textarea
            aria-label="Description"
            value={draft.description}
            onChange={(event) => setDraft((current) => (current ? { ...current, description: event.target.value } : current))}
            placeholder="Reviews implementation quality and regressions."
            rows={3}
          />
        </FormField>
        <FormField label="Avatar" hint="Every agent gets one — pick a different look any time.">
          <button
            type="button"
            aria-label={draft.avatarPet ? "Change avatar" : "Choose an avatar"}
            onClick={() => setPetPickerOpen(true)}
            className="flex items-center gap-2.5 rounded-md border border-border px-3 py-2 text-left hover:bg-accent"
          >
            <AgentAvatar pet={draft.avatarPet} size={28} />
            <span aria-hidden className="text-[12.5px] font-medium">
              {draft.avatarPet ? "Change avatar" : "Choose an avatar"}
            </span>
          </button>
        </FormField>
      </ModalBody>
      <ModalFooter>
        <Button variant="outline" onClick={onClose} disabled={saving}>
          Cancel
        </Button>
        <Button onClick={() => void create()} disabled={!valid || saving}>
          {saving ? "Creating…" : "Create"}
        </Button>
      </ModalFooter>
      <PetPicker
        open={petPickerOpen}
        onClose={() => setPetPickerOpen(false)}
        currentPet={draft.avatarPet}
        onSelect={(avatarPet) => setDraft((current) => (current ? { ...current, avatarPet } : current))}
      />
    </Modal>
  );
}
