import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, FolderOpen, Pencil, Plus, Trash2, Upload } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { ErrorInfo, SkillList } from "../lib/types";
import { toErrorInfo } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

const TEMPLATE = (name: string) => `---
name: ${name}
description: What this skill does and when to use it, in one or two sentences.
---

# ${name}

Step-by-step instructions for the model.
`;

/** The SKILL.md editor with the strict validator (10). */
function SkillEditor(props: {
  folder: string | null;
  onClose: () => void;
  onSaved: () => void;
  onError: (e: ErrorInfo) => void;
}) {
  const isNew = props.folder === null;
  const [folder, setFolder] = useState(props.folder ?? "");
  const [text, setText] = useState(isNew ? TEMPLATE("my-skill") : "");
  const [problems, setProblems] = useState<string[] | null>(null);

  useEffect(() => {
    if (props.folder)
      void ipc
        .getSkillSource(props.folder)
        .then(setText)
        .catch((e) => props.onError(toErrorInfo(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.folder]);

  async function validate() {
    setProblems(await ipc.validateSkill(text, folder));
  }

  async function save() {
    try {
      const p = await ipc.saveSkill(folder, text, isNew);
      if (p.length > 0) setProblems(p);
      else props.onSaved();
    } catch (e) {
      props.onError(toErrorInfo(e));
    }
  }

  return (
    <div className="space-y-2 rounded-lg border border-border p-3">
      <h3 className="font-medium">{isNew ? "New skill" : `Edit ${props.folder}`}</h3>
      {isNew && (
        <label className="block text-sm">
          Folder name (must match <code>name</code>; lowercase letters, digits and hyphens)
          <input
            value={folder}
            onChange={(e) => setFolder(e.target.value)}
            className="mt-1 block w-full rounded-md border border-border bg-bg px-2 py-1 font-mono text-sm"
          />
        </label>
      )}
      <textarea
        aria-label="SKILL.md"
        value={text}
        onChange={(e) => (setText(e.target.value), setProblems(null))}
        rows={16}
        spellCheck={false}
        className="w-full rounded-md border border-border bg-bg p-2 font-mono text-xs"
      />
      {problems !== null &&
        (problems.length === 0 ? (
          <p className="text-sm text-success">No problems found.</p>
        ) : (
          <ul className="list-disc pl-5 text-sm text-danger">
            {problems.map((p, i) => (
              <li key={i}>{p}</li>
            ))}
          </ul>
        ))}
      <div className="flex gap-2">
        <button type="button" onClick={() => void validate()} className="rounded-md border border-border px-3 py-1 text-sm">
          Validate
        </button>
        <button
          type="button"
          disabled={!folder}
          onClick={() => void save()}
          className="rounded-md bg-accent px-3 py-1 text-sm text-accent-fg disabled:opacity-40"
        >
          Save
        </button>
        <button type="button" onClick={props.onClose} className="px-3 py-1 text-sm">
          Cancel
        </button>
      </div>
    </div>
  );
}

export function SkillsSettings() {
  const [list, setList] = useState<SkillList | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [editing, setEditing] = useState<string | null | undefined>(undefined);

  const refresh = useCallback(() => {
    void ipc
      .listSkills()
      .then(setList)
      .catch((e) => setError(toErrorInfo(e)));
  }, []);
  useEffect(refresh, [refresh]);

  const act = (f: () => Promise<unknown>) =>
    void f()
      .then(refresh)
      .catch((e) => setError(toErrorInfo(e)));

  return (
    <div className="space-y-3">
      <h2 className="text-lg font-medium">Skills</h2>
      <p className="text-sm text-fg-muted">
        A skill is a folder with a SKILL.md: instructions the model loads when a task needs them. Only the name and
        description of enabled skills are in every prompt. Scripts in a skill run only through the terminal tool, with
        its approval cards.
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          onClick={() => act(() => ipc.importSkill(false))}
          className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-sm"
        >
          <FolderOpen size={14} aria-hidden /> Import folder…
        </button>
        <button
          type="button"
          onClick={() => act(() => ipc.importSkill(true))}
          className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-sm"
        >
          <Upload size={14} aria-hidden /> Import zip…
        </button>
        <button
          type="button"
          onClick={() => setEditing(null)}
          className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-sm"
        >
          <Plus size={14} aria-hidden /> New skill
        </button>
        <button
          type="button"
          onClick={() => act(() => ipc.openSkillsFolder())}
          className="px-2 py-1 text-sm text-fg-muted underline"
        >
          Open skills folder
        </button>
      </div>
      {editing !== undefined && (
        <SkillEditor
          folder={editing}
          onClose={() => setEditing(undefined)}
          onSaved={() => {
            setEditing(undefined);
            refresh();
          }}
          onError={setError}
        />
      )}
      <label className="flex items-start gap-2 text-sm">
        <input
          type="checkbox"
          className="mt-1"
          checked={list?.sharedEnabled ?? false}
          onChange={(e) => act(() => ipc.setSharedSkills(e.target.checked))}
        />
        <span>
          Also use skills from {list?.sharedPath ?? "~/.agents/skills"} (shared with other agent apps; read-only here)
        </span>
      </label>
      <ul className="space-y-2">
        {list?.skills.length === 0 && <li className="text-sm text-fg-muted">No skills yet.</li>}
        {list?.skills.map((s) => (
          <li key={s.name} className="rounded-lg border border-border p-3 text-sm">
            <div className="flex items-start gap-2">
              <input
                type="checkbox"
                className="mt-1"
                aria-label={`Enable ${s.name}`}
                checked={s.enabled}
                onChange={(e) => act(() => ipc.setSkillEnabled(s.name, e.target.checked))}
              />
              <div className="min-w-0 flex-1">
                <div className="font-medium">
                  {s.name}
                  {s.shared && <span className="ml-2 text-xs font-normal text-fg-muted">shared folder</span>}
                  {s.license && <span className="ml-2 text-xs font-normal text-fg-muted">{s.license}</span>}
                </div>
                <p className="text-fg-muted">{s.description}</p>
                {s.allowedTools.length > 0 && (
                  <p className="text-xs text-fg-muted">
                    Lists tools: {s.allowedTools.join(", ")} (informational; it grants nothing)
                  </p>
                )}
                {s.warnings.map((w, i) => (
                  <p key={i} className="flex items-center gap-1 text-xs text-warning">
                    <AlertTriangle size={12} aria-hidden /> {w}
                  </p>
                ))}
              </div>
              {!s.shared && (
                <>
                  <button type="button" title="Edit" aria-label={`Edit ${s.name}`} onClick={() => setEditing(s.name)}>
                    <Pencil size={14} aria-hidden />
                  </button>
                  <button
                    type="button"
                    title="Delete"
                    aria-label={`Delete ${s.name}`}
                    onClick={() => {
                      if (window.confirm(`Delete the skill ${s.name} and its folder?`)) act(() => ipc.deleteSkill(s.name));
                    }}
                  >
                    <Trash2 size={14} aria-hidden />
                  </button>
                </>
              )}
            </div>
          </li>
        ))}
      </ul>
      {list && list.problems.length > 0 && (
        <div className="text-sm">
          <h3 className="font-medium">Not loaded</h3>
          <ul className="list-disc pl-5 text-fg-muted">
            {list.problems.map((p) => (
              <li key={p.path}>
                <span className="font-mono text-xs">{p.path}</span>: {p.error}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
