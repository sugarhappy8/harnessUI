# Prints the raw results of the project's code for fixed inputs, as one line
# "RESULT <json>". It decides nothing: the app compares the values with what it
# expects. It runs in its own process; the app never loads the project's code.
import json
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.getcwd())


def case(fn):
    try:
        return {"value": fn()}
    except Exception as e:  # noqa: BLE001
        return {"error": type(e).__name__}


def done(fn):
    try:
        fn()
        return "ok"
    except Exception as e:  # noqa: BLE001
        return type(e).__name__


def run():
    from todo import TodoList

    todos = TodoList()
    for title in ["a", "b", "c"]:
        todos.add(title)
    out = {"start": case(todos.pending)}
    out["completeB"] = done(lambda: todos.complete("b"))
    out["afterB"] = case(todos.pending)
    out["titles"] = case(todos.titles)
    out["completeRest"] = done(lambda: (todos.complete("a"), todos.complete("c")))
    out["allDone"] = case(todos.pending)
    other = TodoList()
    other.add("a")
    out["missing"] = done(lambda: other.complete("missing"))
    return out
if __name__ == "__main__":
    try:
        results = run()
    except Exception as e:  # noqa: BLE001
        results = {"failedToLoad": type(e).__name__}
    print("RESULT " + json.dumps(results, default=repr), flush=True)
