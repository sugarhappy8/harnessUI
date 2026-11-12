import { useEffect, useState } from "react";
import { CheckCheck, Pause, Play, RotateCcw, Trash2 } from "lucide-react";
import { useApp } from "../../store/app";
import * as api from "../task/api";
import { ClearTaskDialog } from "../task/ClearTaskDialog";
import { NEXT_STATUS, ConstraintsEditor, StepsEditor, TextEditor } from "../task/TaskEditors";
import { ChangedFiles, Checks } from "../task/TaskFacts";
import { ByYou, LinkButton, Section, STEP_LABEL, STEP_MARK, StatusChip, when } from "../task/TaskParts";
import { toProblem, useTasks } from "../task/state";
import type { Task, TaskEdit, TaskStatus } from "../task/types";

/** The message Resume sends: an ordinary message, through the ordinary send path. */
export const RESUME_MESSAGE = "Continue the task. Start with the next action.";

type Editing = "goal" | "constraints" | "steps" | "next" | null;

function headerButton(tone: "plain" | "accent" = "plain"): string {
  return (
    "inline-flex items-center gap-1 rounded-md border px-2 py-1 text-xs disabled:opacity-50 " +
    (tone === "accent" ? "border-accent/60 bg-accent/10 text-accent hover:bg-accent/20" : "border-border hover:bg-bg-muted")
  );
}

/** The "Task" tab of the panel beside the chat: the goal, constraints, steps, changed files,
 *  checks and next action of the work in this chat, with Pause, Resume, Mark done and
 *  Clear. The user corrects the goal, constraints, steps and next action in place. */
export function TaskTab(props: { chatId: number }) {
  const { chatId } = props;
  const task = useTasks((s) => s.byChat[chatId]);
  const problem = useTasks((s) => s.problems[chatId]);
  const generation = useApp((s) => s.generation);
  const activeChatId = useApp((s) => s.activeChatId);
  // What is being edited, for the chat it was started in.
  const [edited, setEdited] = useState<{ chatId: number; what: Editing }>({ chatId, what: null });
  const editing = edited.chatId === chatId ? edited.what : null;
  const setEditing = (what: Editing) => setEdited({ chatId, what });
  const [busy, setBusy] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);

  useEffect(() => {
    void useTasks.getState().load(chatId);
  }, [chatId]);

  const runningHere = generation !== null && generation.chatId === chatId;
  const anyRunning = generation !== null;

  async function act(f: () => Promise<Task | null | void>): Promise<boolean> {
    setBusy(true);
    try {
      const result = await f();
      if (result !== undefined) useTasks.getState().put(chatId, result);
      return true;
    } catch (err) {
      useTasks.getState().setProblem(chatId, toProblem(err));
      return false;
    } finally {
      setBusy(false);
    }
  }

  const edit = (change: TaskEdit) =>
    void act(() => api.editTask(chatId, change)).then((ok) => {
      if (ok) setEditing(null);
    });

  const setStatus = (status: TaskStatus) => act(() => api.setTaskStatus(chatId, status));

  async function pause() {
    if (!(await setStatus("paused"))) return;
    // The same stop as the Stop button, only for a turn of this chat.
    const g = useApp.getState().generation;
    if (g && g.chatId === chatId) await useApp.getState().stop();
  }

  async function resume() {
    if (!(await setStatus("active"))) return;
    if (useApp.getState().activeChatId === chatId && useApp.getState().generation === null) {
      await useApp.getState().send(RESUME_MESSAGE);
    }
  }

  async function clear() {
    if (await act(() => api.clearTask(chatId))) useTasks.getState().put(chatId, null);
  }

  const problemNote = problem && (
    <div role="alert" className="mx-3 mt-3 rounded-lg border border-warning/50 bg-warning/10 p-2 text-xs">
      <p>{problem.message}</p>
      {problem.code === "task_unreadable" && (
        <button type="button" className={headerButton() + " mt-1.5"} onClick={() => setConfirmClear(true)}>
          <Trash2 size={12} aria-hidden /> Clear task
        </button>
      )}
    </div>
  );
  const dialog = (
    <ClearTaskDialog open={confirmClear} onOpenChange={setConfirmClear} onConfirm={() => void clear()} />
  );

  if (task === undefined && !problem) {
    return <p className="p-6 text-sm text-fg-muted" data-chat={chatId}>Reading the task…</p>;
  }

  if (!task) {
    return (
      <div data-chat={chatId}>
        {problemNote}
        <div className="space-y-3 p-6 text-sm text-fg-muted">
          <p>
            No task in this chat yet. When the model records one, or you write a goal, its goal, steps, changed files
            and checks appear here, and the work can be paused and picked up again later.
          </p>
          {editing === "goal" ? (
            <TextEditor
              label="Goal"
              value=""
              multiline
              busy={busy}
              onApply={(goal) => edit({ goal })}
              onCancel={() => setEditing(null)}
            />
          ) : (
            <LinkButton label="Write a goal" onClick={() => setEditing("goal")} />
          )}
        </div>
        {dialog}
      </div>
    );
  }

  const canResume = task.status === "paused" || (task.status === "active" && !runningHere);

  return (
    <div className="space-y-3 p-3" data-chat={chatId} data-task-status={task.status}>
      <header className="flex flex-wrap items-center gap-2 px-1">
        <h2 className="text-sm font-semibold">Task</h2>
        <StatusChip status={task.status} />
        <div className="ml-auto flex flex-wrap gap-1.5">
          {task.status === "active" && (
            <button type="button" className={headerButton()} disabled={busy} onClick={() => void pause()}>
              <Pause size={12} aria-hidden /> Pause
            </button>
          )}
          {task.status !== "done" && canResume && (
            <button
              type="button"
              className={headerButton("accent")}
              disabled={busy || anyRunning || activeChatId !== chatId}
              title={anyRunning ? "Wait until the current reply ends." : undefined}
              onClick={() => void resume()}
            >
              <Play size={12} aria-hidden /> Resume
            </button>
          )}
          {task.status !== "done" ? (
            <button type="button" className={headerButton()} disabled={busy} onClick={() => void setStatus("done")}>
              <CheckCheck size={12} aria-hidden /> Mark done
            </button>
          ) : (
            <button type="button" className={headerButton()} disabled={busy} onClick={() => void setStatus("active")}>
              <RotateCcw size={12} aria-hidden /> Reopen
            </button>
          )}
          <button type="button" className={headerButton()} disabled={busy} onClick={() => setConfirmClear(true)}>
            <Trash2 size={12} aria-hidden /> Clear task
          </button>
        </div>
      </header>
      {problemNote}
      {task.status === "paused" && (
        <p className="px-1 text-xs text-fg-muted">
          Paused. Correct anything below, then choose Resume to continue from the next action.
        </p>
      )}

      <Section
        title="Goal"
        action={editing !== "goal" && <LinkButton label="Edit" ariaLabel="Edit the goal" onClick={() => setEditing("goal")} />}
      >
        {editing === "goal" ? (
          <TextEditor
            label="Goal"
            value={task.goal}
            multiline
            busy={busy}
            onApply={(goal) => edit({ goal })}
            onCancel={() => setEditing(null)}
          />
        ) : task.goal ? (
          <p className="break-words" data-field="goal">
            {task.goal}
            {task.goalByUser && <ByYou />}
          </p>
        ) : (
          <p className="text-xs text-fg-muted">Not set.</p>
        )}
      </Section>

      <Section
        title="Constraints"
        action={
          editing !== "constraints" && (
            <LinkButton label="Edit" ariaLabel="Edit the constraints" onClick={() => setEditing("constraints")} />
          )
        }
      >
        {editing === "constraints" ? (
          <ConstraintsEditor
            value={task.constraints.map((c) => c.text)}
            busy={busy}
            onApply={(constraints) => edit({ constraints })}
            onCancel={() => setEditing(null)}
          />
        ) : task.constraints.length > 0 ? (
          <ul className="list-disc space-y-0.5 pl-5">
            {task.constraints.map((c) => (
              <li key={c.text} className="break-words">
                {c.text}
                {c.byUser && <ByYou />}
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-xs text-fg-muted">None.</p>
        )}
      </Section>

      <Section
        title="Steps"
        action={editing !== "steps" && <LinkButton label="Edit" ariaLabel="Edit the steps" onClick={() => setEditing("steps")} />}
      >
        {editing === "steps" ? (
          <StepsEditor
            value={task.steps}
            busy={busy}
            onApply={(steps) => edit({ steps })}
            onCancel={() => setEditing(null)}
          />
        ) : task.steps.length > 0 ? (
          <ol className="space-y-0.5" aria-label="Steps">
            {task.steps.map((s, i) => (
              <li key={`${i}:${s.text}`} className="flex items-start gap-2" data-step-status={s.status}>
                <button
                  type="button"
                  disabled={busy}
                  className="w-5 shrink-0 text-center text-accent"
                  aria-label={`Step ${i + 1} is ${STEP_LABEL[s.status]}. Change it.`}
                  onClick={() =>
                    edit({
                      steps: task.steps.map((t, j) => ({
                        text: t.text,
                        status: j === i ? NEXT_STATUS[t.status] : t.status,
                      })),
                    })
                  }
                >
                  {STEP_MARK[s.status]}
                </button>
                <span className={"min-w-0 break-words " + (s.status === "done" ? "text-fg-muted line-through" : "")}>
                  {s.text}
                  {s.byUser && <ByYou />}
                </span>
              </li>
            ))}
          </ol>
        ) : (
          <p className="text-xs text-fg-muted">None.</p>
        )}
      </Section>

      <Section
        title="Next action"
        action={
          editing !== "next" && <LinkButton label="Edit" ariaLabel="Edit the next action" onClick={() => setEditing("next")} />
        }
      >
        {editing === "next" ? (
          <TextEditor
            label="Next action"
            value={task.nextAction}
            busy={busy}
            onApply={(nextAction) => edit({ nextAction })}
            onCancel={() => setEditing(null)}
          />
        ) : task.nextAction ? (
          <p className="break-words" data-field="next-action">
            {task.nextAction}
            {task.nextActionByUser && <ByYou />}
          </p>
        ) : (
          <p className="text-xs text-fg-muted">Not set.</p>
        )}
      </Section>

      <Section title="Changed files">
        <ChangedFiles files={task.changedFiles} notListed={task.filesNotListed} />
        <p className="mt-1.5 text-[11px] text-fg-muted">
          Only changes made with the file tools are listed. Files a command changes are not tracked.
        </p>
      </Section>

      <Section title="Checks">
        <Checks checks={task.checks} />
      </Section>

      {task.notes.length > 0 && (
        <Section title="Notes from the model">
          <ul className="space-y-1 text-xs">
            {task.notes.map((n) => (
              <li key={`${n.at}:${n.text}`} className="break-words">
                {n.text} <span className="text-fg-muted">{when(n.at)}</span>
              </li>
            ))}
          </ul>
        </Section>
      )}

      <p className="px-1 text-xs text-fg-muted">Last updated {task.updatedAt > 0 ? when(task.updatedAt) : "never"}.</p>
      {dialog}
    </div>
  );
}
