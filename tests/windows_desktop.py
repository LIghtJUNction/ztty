#!/usr/bin/env python3
"""Run the unchanged Windows native gate on a temporary, visible CI desktop.

Only GitHub's named Windows runners may opt in with --isolated-desktop. The
controller keeps the original desktop open, starts a suspended worker with an
explicit stdio handle list, assigns its own kill-on-close Job, then resumes it.
No window belonging to another process is moved, hidden, closed, or terminated.

Win32 contracts:
https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-switchdesktop
https://learn.microsoft.com/windows/win32/api/processthreadsapi/ns-processthreadsapi-startupinfow
https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute
https://learn.microsoft.com/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject
https://learn.microsoft.com/windows/win32/procthread/job-objects
"""
import argparse
import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import traceback
import uuid


# Fixed-width Win32 types also allow portable ABI/cleanup contract checks.
DWORD, BOOL, HANDLE = C.c_uint32, C.c_int32, C.c_void_p
SIZE_T, LPWSTR = C.c_size_t, C.c_wchar_p
READ_OBJECTS, SWITCH_DESKTOP = 0x0001, 0x0100
CREATE_SUSPENDED, EXTENDED_STARTUPINFO_PRESENT, CREATE_NO_WINDOW = 4, 0x80000, 0x08000000
CREATION_FLAGS = CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW
KILL_ON_JOB_CLOSE, HANDLE_LIST = 0x2000, 0x20002
WAIT_TIMEOUT, WAIT_FAILED = 258, 0xFFFFFFFF


class SecurityAttributes(C.Structure):
    _fields_ = [("length", DWORD), ("descriptor", C.c_void_p), ("inherit", BOOL)]


class StartupInfo(C.Structure):
    _fields_ = [("cb", DWORD), ("reserved", LPWSTR), ("desktop", LPWSTR),
                ("title", LPWSTR), ("x", DWORD), ("y", DWORD),
                ("x_size", DWORD), ("y_size", DWORD), ("x_chars", DWORD),
                ("y_chars", DWORD), ("fill", DWORD), ("flags", DWORD),
                ("show", C.c_uint16), ("reserved_size", C.c_uint16),
                ("reserved_bytes", C.c_void_p), ("stdin", HANDLE),
                ("stdout", HANDLE), ("stderr", HANDLE)]


class StartupInfoEx(C.Structure):
    _fields_ = [("startup", StartupInfo), ("attributes", C.c_void_p)]


class ProcessInfo(C.Structure):
    _fields_ = [("process", HANDLE), ("thread", HANDLE), ("pid", DWORD), ("tid", DWORD)]


class JobBasicLimit(C.Structure):
    _fields_ = [("process_time", C.c_int64), ("job_time", C.c_int64),
                ("flags", DWORD), ("min_working_set", SIZE_T),
                ("max_working_set", SIZE_T), ("active_process_limit", DWORD),
                ("affinity", SIZE_T), ("priority", DWORD), ("scheduling", DWORD)]


class JobExtendedLimit(C.Structure):
    _fields_ = [("basic", JobBasicLimit), ("io_counters", C.c_uint64 * 6),
                ("process_memory_limit", SIZE_T), ("job_memory_limit", SIZE_T),
                ("peak_process_memory", SIZE_T), ("peak_job_memory", SIZE_T)]


class UserObjectFlags(C.Structure):
    _fields_ = [("inherit", BOOL), ("reserved", BOOL), ("flags", DWORD)]


class DesktopBlocked(RuntimeError):
    """This runner cannot safely provide the requested visible test desktop."""


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def require_context(state, desktop="Default"):
    if (state.get("station", "").casefold() != "winsta0"
            or state.get("visible") is not True
            or state.get("thread", "").casefold() != desktop.casefold()
            or state.get("input", "").casefold() != desktop.casefold()):
        raise DesktopBlocked(f"expected visible WinSta0/{desktop} thread and input desktop; got {state}")


class WindowsAPI:
    def __init__(self):
        if sys.platform != "win32":
            raise DesktopBlocked("desktop isolation requires native Windows")
        self.user = C.WinDLL("user32", use_last_error=True)
        self.kernel = C.WinDLL("kernel32", use_last_error=True)
        for library, declarations in (
            (self.user, {
                "GetProcessWindowStation": (HANDLE, []),
                "GetThreadDesktop": (HANDLE, [DWORD]),
                "GetUserObjectInformationW": (BOOL, [HANDLE, C.c_int, C.c_void_p, DWORD, C.POINTER(DWORD)]),
                "OpenInputDesktop": (HANDLE, [DWORD, BOOL, DWORD]),
                "CreateDesktopW": (HANDLE, [LPWSTR, LPWSTR, C.c_void_p, DWORD, DWORD, C.c_void_p]),
                "SwitchDesktop": (BOOL, [HANDLE]),
                "SetThreadDesktop": (BOOL, [HANDLE]),
                "CloseDesktop": (BOOL, [HANDLE]),
            }),
            (self.kernel, {
                "GetCurrentThreadId": (DWORD, []),
                "CreateJobObjectW": (HANDLE, [C.c_void_p, LPWSTR]),
                "SetInformationJobObject": (BOOL, [HANDLE, C.c_int, C.c_void_p, DWORD]),
                "QueryInformationJobObject": (BOOL, [HANDLE, C.c_int, C.c_void_p, DWORD, C.POINTER(DWORD)]),
                "AssignProcessToJobObject": (BOOL, [HANDLE, HANDLE]),
                "IsProcessInJob": (BOOL, [HANDLE, HANDLE, C.POINTER(BOOL)]),
                "TerminateJobObject": (BOOL, [HANDLE, DWORD]),
                "CreateFileW": (HANDLE, [LPWSTR, DWORD, DWORD, C.POINTER(SecurityAttributes), DWORD, DWORD, HANDLE]),
                "InitializeProcThreadAttributeList": (BOOL, [C.c_void_p, DWORD, DWORD, C.POINTER(SIZE_T)]),
                "UpdateProcThreadAttribute": (BOOL, [C.c_void_p, DWORD, SIZE_T, C.c_void_p, SIZE_T, C.c_void_p, C.c_void_p]),
                "DeleteProcThreadAttributeList": (None, [C.c_void_p]),
                "CreateProcessW": (BOOL, [LPWSTR, LPWSTR, C.c_void_p, C.c_void_p, BOOL, DWORD,
                                         C.c_void_p, LPWSTR, C.POINTER(StartupInfoEx), C.POINTER(ProcessInfo)]),
                "ResumeThread": (DWORD, [HANDLE]),
                "WaitForSingleObject": (DWORD, [HANDLE, DWORD]),
                "GetExitCodeProcess": (BOOL, [HANDLE, C.POINTER(DWORD)]),
                "TerminateProcess": (BOOL, [HANDLE, DWORD]),
                "CloseHandle": (BOOL, [HANDLE]),
            }),
        ):
            for name, (result, arguments) in declarations.items():
                function = getattr(library, name)
                function.restype, function.argtypes = result, arguments

    @staticmethod
    def checked(result, operation):
        if not result:
            error = C.get_last_error()
            raise OSError(error, f"{operation}: {C.FormatError(error).strip()}")
        return result

    def name(self, handle):
        value, needed = C.create_unicode_buffer(256), DWORD()
        self.checked(handle, "desktop/window station handle")
        self.checked(self.user.GetUserObjectInformationW(
            handle, 2, value, C.sizeof(value), C.byref(needed)), "GetUserObjectInformationW(UOI_NAME)")
        return value.value

    def thread_desktop(self):
        return self.checked(self.user.GetThreadDesktop(self.kernel.GetCurrentThreadId()), "GetThreadDesktop")

    def open_input(self, switch=False):
        return self.checked(self.user.OpenInputDesktop(
            0, False, READ_OBJECTS | (SWITCH_DESKTOP if switch else 0)), "OpenInputDesktop")

    def input_name(self):
        handle = self.open_input()
        try:
            return self.name(handle)
        finally:
            self.close_desktop(handle)

    def context(self):
        station = self.user.GetProcessWindowStation()
        flags, needed = UserObjectFlags(), DWORD()
        self.checked(self.user.GetUserObjectInformationW(
            station, 1, C.byref(flags), C.sizeof(flags), C.byref(needed)),
            "GetUserObjectInformationW(UOI_FLAGS)")
        return {"station": self.name(station), "visible": bool(flags.flags & 1),
                "thread": self.name(self.thread_desktop()), "input": self.input_name()}

    def create_desktop(self, name):
        # READOBJECTS, CREATEWINDOW, CREATEMENU, ENUMERATE, WRITEOBJECTS, SWITCHDESKTOP.
        # A null security descriptor uses the existing window station's policy.
        return self.checked(self.user.CreateDesktopW(
            name, None, None, 0, 0x01C7, None), "CreateDesktopW")

    def switch(self, handle):
        # SwitchDesktop does not always set LastError on failure; clear stale state.
        C.set_last_error(0)
        self.checked(self.user.SwitchDesktop(handle), "SwitchDesktop")

    def restore_thread(self, handle):
        self.checked(self.user.SetThreadDesktop(handle), "SetThreadDesktop(original)")

    def close_desktop(self, handle):
        self.checked(self.user.CloseDesktop(handle), "CloseDesktop")

    def create_job(self):
        return self.checked(self.kernel.CreateJobObjectW(None, None), "CreateJobObjectW")

    def limit_job(self, job):
        limits = JobExtendedLimit()
        limits.basic.flags = KILL_ON_JOB_CLOSE
        self.checked(self.kernel.SetInformationJobObject(
            job, 9, C.byref(limits), C.sizeof(limits)), "SetInformationJobObject(KILL_ON_JOB_CLOSE)")

    def assign(self, job, process):
        self.checked(self.kernel.AssignProcessToJobObject(job, process.process), "AssignProcessToJobObject")
        present = BOOL()
        self.checked(self.kernel.IsProcessInJob(process.process, job, C.byref(present)), "IsProcessInJob")
        if not present.value:
            raise DesktopBlocked("created worker was not assigned to its owned Job")

    def job_pids(self, job):
        capacity = 16
        while capacity <= 4096:
            class ProcessIds(C.Structure):
                _fields_ = [("assigned", DWORD), ("count", DWORD), ("pids", SIZE_T * capacity)]
            value = ProcessIds()
            ok = self.kernel.QueryInformationJobObject(job, 3, C.byref(value), C.sizeof(value), None)
            if ok and value.count == value.assigned and value.count <= capacity:
                return sorted(int(pid) for pid in value.pids[:value.count])
            if not ok and C.get_last_error() != 234:  # ERROR_MORE_DATA
                self.checked(ok, "QueryInformationJobObject(ProcessIdList)")
            capacity = max(capacity * 2, value.assigned)
        raise RuntimeError("owned Job process list exceeds its diagnostic bound")

    def create_suspended(self, command, desktop, stdout, stderr, process):
        handles, attributes, initialized = [], None, False
        # process is owned by the caller even if closing a temporary handle fails.
        try:
            security = SecurityAttributes(C.sizeof(SecurityAttributes), None, True)
            for path, access, disposition in (("NUL", 0x80000000, 3),
                                               (str(stdout), 0x40000000, 2),
                                               (str(stderr), 0x40000000, 2)):
                handle = self.kernel.CreateFileW(path, access, 3, C.byref(security), disposition, 0x80, None)
                if handle == C.c_void_p(-1).value:
                    self.checked(False, "CreateFileW(owned stdio)")
                handles.append(handle)
            size = SIZE_T()
            self.kernel.InitializeProcThreadAttributeList(None, 1, 0, C.byref(size))
            if not size.value or size.value > 65536:
                self.checked(False, "InitializeProcThreadAttributeList(size)")
            attributes = C.create_string_buffer(size.value)
            self.checked(self.kernel.InitializeProcThreadAttributeList(
                attributes, 1, 0, C.byref(size)), "InitializeProcThreadAttributeList")
            initialized = True
            inherited = (HANDLE * 3)(*handles)
            self.checked(self.kernel.UpdateProcThreadAttribute(
                attributes, 0, HANDLE_LIST, inherited, C.sizeof(inherited), None, None),
                "UpdateProcThreadAttribute(HANDLE_LIST)")
            startup = StartupInfoEx()
            startup.startup.cb = C.sizeof(startup)
            startup.startup.desktop = "WinSta0\\" + desktop
            startup.startup.flags = 0x100  # STARTF_USESTDHANDLES
            startup.startup.stdin, startup.startup.stdout, startup.startup.stderr = handles
            startup.attributes = C.cast(attributes, C.c_void_p)
            command_line = C.create_unicode_buffer(subprocess.list2cmdline(command))
            self.checked(self.kernel.CreateProcessW(
                command[0], command_line, None, None, True, CREATION_FLAGS,
                None, str(Path.cwd()), C.byref(startup), C.byref(process)), "CreateProcessW(suspended)")
        finally:
            if initialized:
                self.kernel.DeleteProcThreadAttributeList(attributes)
            errors = []
            for handle in handles:
                try:
                    self.close_handle(handle)
                except OSError as error:
                    errors.append(str(error))
            if errors:
                raise OSError("owned stdio handle cleanup failed: " + "; ".join(errors))

    def resume(self, process):
        previous = self.kernel.ResumeThread(process.thread)
        if previous == WAIT_FAILED:
            self.checked(False, "ResumeThread")
        if previous != 1:
            raise RuntimeError(f"unexpected suspended worker count: {previous}")

    def poll(self, process, milliseconds=0):
        result = self.kernel.WaitForSingleObject(process.process, milliseconds)
        if result == WAIT_TIMEOUT:
            return None
        if result == WAIT_FAILED:
            self.checked(False, "WaitForSingleObject(worker)")
        if result != 0:
            raise RuntimeError(f"unexpected worker wait result: {result}")
        code = DWORD()
        self.checked(self.kernel.GetExitCodeProcess(process.process, C.byref(code)), "GetExitCodeProcess")
        return code.value

    def terminate_job(self, job):
        self.checked(self.kernel.TerminateJobObject(job, 1), "TerminateJobObject(owned)")

    def terminate_process(self, process):
        self.checked(self.kernel.TerminateProcess(process.process, 1), "TerminateProcess(owned suspended worker)")

    def close_handle(self, handle):
        self.checked(self.kernel.CloseHandle(handle), "CloseHandle(owned)")


def run_isolated(api, command, desktop, stdout, stderr, timeout, checkpoint=lambda value: None,
                 clock=time.monotonic, pause=time.sleep):
    """Own exactly one desktop, one Job, and the process returned by our CreateProcess."""
    report = {"passed": False, "status": "running", "desktop": desktop,
              "creation_flags": CREATION_FLAGS, "inherited_handles": ["NUL stdin", "stdout log", "stderr log"],
              "kill_on_job_close": False, "worker_resumed": False, "observed_job_pids": [],
              "cleanup": {"errors": [], "job_empty": False, "worker_exited": False,
                          "input_restored": False, "thread_restored": False,
                          "owned_desktop_closed": False, "handles_closed": False}}
    original, own, job, thread = None, None, None, None
    process, assigned = ProcessInfo(), False
    cleanup, observed = report["cleanup"], set()
    started = clock()

    def pids():
        active = api.job_pids(job)
        observed.update(active)
        report["observed_job_pids"] = sorted(observed)
        return active

    def require_input(name):
        actual = api.input_name()
        report["last_input_desktop"] = actual
        if actual.casefold() != name.casefold():
            raise DesktopBlocked(f"input desktop changed from {name} to {actual}; no forced switch attempted")

    def failure(error):
        report["passed"] = False
        unavailable = isinstance(error, DesktopBlocked) or (
            isinstance(error, OSError) and not isinstance(error, TimeoutError))
        report["status"] = "blocked" if unavailable else "failed"
        report["error"] = f"{type(error).__name__}: {error}"

    try:
        report["before"] = api.context()
        require_context(report["before"])
        thread = api.thread_desktop()  # Borrowed handle: never CloseDesktop on it.
        original = api.open_input(switch=True)
        if api.name(original).casefold() != "default":
            raise DesktopBlocked("input desktop changed before acquiring the original handle")
        own = api.create_desktop(desktop)
        if api.name(own) != desktop:
            raise DesktopBlocked("CreateDesktop returned an unexpected desktop")
        # CreateDesktop's thread association is explicit in the API contract.
        if api.name(api.thread_desktop()) == desktop:
            api.restore_thread(thread)
        require_context(api.context())
        report["created_desktop"] = True
        checkpoint(report)
        job = api.create_job()
        api.limit_job(job)
        report["kill_on_job_close"] = True
        api.create_suspended(command, desktop, stdout, stderr, process)
        report["worker_pid"], report["worker_tid"] = process.pid, process.tid
        report["created_suspended"] = True
        checkpoint(report)
        api.assign(job, process)
        assigned = report["worker_assigned"] = True
        report["assigned_job_pids"] = pids()
        if process.pid not in report["assigned_job_pids"]:
            raise DesktopBlocked("owned Job did not report the suspended worker")
        require_input("Default")
        api.switch(own)
        require_input(desktop)
        report["switched_to_owned"] = True
        checkpoint(report)
        api.resume(process)
        report["worker_resumed"] = True
        checkpoint(report)
        deadline = clock() + timeout
        while True:
            require_input(desktop)
            pids()
            code = api.poll(process, 200)
            if code is not None:
                report["worker_exit_code"] = code
                break
            if clock() >= deadline:
                raise TimeoutError(f"native worker exceeded {timeout:g} seconds")
        if code != 0:
            raise RuntimeError(f"native worker exited with {code}; see its JSON and stderr log")
        report["passed"], report["status"] = True, "passed"
    except BaseException as error:
        failure(error)
    finally:
        # No PID search/kill: terminate only our Job or our unassigned suspended handle.
        try:
            if process.process and api.poll(process) is None:
                cleanup["forced_termination"] = True
                if assigned:
                    api.terminate_job(job)
                else:
                    api.terminate_process(process)
            deadline = clock() + 5
            active = pids() if job else []
            while active and clock() < deadline:
                pause(0.05)
                active = pids()
            if active:
                cleanup["lingering_job_pids"] = active
                cleanup["forced_termination"] = True
                cleanup["errors"].append("owned Job retained child processes after worker exit")
                api.terminate_job(job)
                deadline = clock() + 5
                while active and clock() < deadline:
                    pause(0.05)
                    active = pids()
            cleanup["job_pids_after"] = active
            cleanup["job_empty"] = not active
            cleanup["worker_exited"] = not process.process or api.poll(process, 5000) is not None
            if not cleanup["job_empty"] or not cleanup["worker_exited"]:
                raise RuntimeError("owned worker/Job exit was not verified")
        except BaseException as error:
            cleanup["errors"].append(f"process cleanup: {error}")
        try:
            current = api.input_name()
            cleanup["input_before_restore"] = current
            if own and current == desktop:
                if not original:
                    raise DesktopBlocked("original input handle is unavailable")
                api.switch(original)
            elif current.casefold() != "default":
                raise DesktopBlocked(f"input is {current}; refusing to switch an unrelated or secure desktop")
            cleanup["input_after_restore"] = api.input_name()
            cleanup["input_restored"] = cleanup["input_after_restore"].casefold() == "default"
            if not cleanup["input_restored"]:
                raise DesktopBlocked("original input desktop was not restored")
        except BaseException as error:
            cleanup["errors"].append(f"input restoration: {error}")
            report["status"] = "blocked"
        try:
            current = api.name(api.thread_desktop())
            if thread and own and current == desktop:
                api.restore_thread(thread)
            cleanup["thread_restored"] = api.name(api.thread_desktop()).casefold() == "default"
            if not cleanup["thread_restored"]:
                raise DesktopBlocked("controller thread desktop was not restored")
        except BaseException as error:
            cleanup["errors"].append(f"thread restoration: {error}")
        # Keep the kill-on-close Job uninherited and close it even if query/termination failed.
        close_errors = []
        for label, handle in (("worker thread", process.thread), ("worker process", process.process), ("owned Job", job)):
            if handle:
                try:
                    api.close_handle(handle)
                except BaseException as error:
                    close_errors.append(f"{label}: {error}")
        cleanup["handles_closed"] = not close_errors
        cleanup["errors"].extend(close_errors)
        if own:
            try:
                if not cleanup["input_restored"] or not cleanup["thread_restored"]:
                    raise DesktopBlocked("owned desktop cannot be closed before verified restoration")
                api.close_desktop(own)
                cleanup["owned_desktop_closed"] = True
            except BaseException as error:
                cleanup["errors"].append(f"owned desktop close: {error}")
        else:
            cleanup["owned_desktop_closed"] = True
        if original:
            try:
                api.close_desktop(original)
            except BaseException as error:
                cleanup["errors"].append(f"original input handle close: {error}")
        try:
            report["after"] = api.context()
        except BaseException as error:
            report["after"] = {"error": str(error)}
        if cleanup["errors"] or cleanup.get("forced_termination"):
            report["passed"] = False
            if report["status"] != "blocked":
                report["status"] = "failed"
        report["elapsed_seconds"] = round(clock() - started, 3)
        checkpoint(report)
    return report


def combine(native, desktop, worker):
    """A desktop/cleanup success cannot replace the existing visible pixel gate."""
    from windows_workspace_native import validate_report
    checks = native.get("checks")
    native_ok = (native.get("passed") is True and native.get("status") == "passed"
                 and isinstance(checks, list) and bool(checks)
                 and all(check.get("passed") is True for check in checks)
                 and native.get("visible_pixel_checks_completed", 0) > 0
                 and native.get("cleanup", {}).get("process_exited") is True
                 and native.get("cleanup", {}).get("temporary_directory_removed") is True
                 and validate_report(native.get("workspace", {})))
    worker_ok = (worker.get("passed") is True and worker.get("pid") == desktop.get("worker_pid")
                 and worker.get("desktop") == desktop.get("desktop"))
    try:
        require_context(worker.get("context", {}), desktop["desktop"])
        if native_ok:
            for suite in (native, native["workspace"]):
                for key in ("thread", "input"):
                    if suite.get("desktop", {}).get(key, {}).get("name") != desktop["desktop"]:
                        raise DesktopBlocked("native pixel gate did not observe its owned input/thread desktop")
    except (DesktopBlocked, KeyError):
        worker_ok = False
    passed = native_ok and worker_ok and desktop.get("passed") is True
    status = "passed" if passed else (
        "blocked" if "blocked" in (native.get("status"), desktop.get("status"), worker.get("status")) else "failed")
    native.update({"passed": passed, "status": status, "desktop_isolation": desktop})
    if not passed and "error" not in native:
        native["error"] = desktop.get("error", "native report, worker desktop, or cleanup evidence did not pass")
    return native


def require_ci(opt_in):
    if (not opt_in or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("RUNNER_OS") != "Windows"
            or os.environ.get("RUNNER_LABEL") not in ("windows-2025", "windows-11-arm")):
        raise DesktopBlocked("isolated desktop requires explicit opt-in on the named GitHub Windows CI runners")


def worker(args, state_path):
    state = {"passed": False, "status": "blocked", "pid": os.getpid(), "desktop": args.worker_desktop}
    try:
        api = WindowsAPI()
        state["context"] = api.context()
        require_context(state["context"], args.worker_desktop)
        state["passed"], state["status"] = True, "passed"
    except BaseException as error:
        state["error"] = str(error)
        write_json(args.output, {"passed": False, "status": "blocked", "error": str(error), "checks": []})
        return 2
    finally:
        write_json(state_path, state)
    import windows_native
    try:
        if not windows_native.run(args.binary, args.output, args.arch):
            return 1
    except BaseException:
        # windows_native.run writes its checks, original error, PNGs and cleanup
        # from finally. Preserve that evidence instead of replacing its report.
        traceback.print_exc()
        try:
            original = json.loads(args.output.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return 1
        cleanup = original.get("cleanup", {})
        if (original.get("passed") is False
                and original.get("failed_pixel", {}).get("name") == "resize_restores_visible_image"
                and cleanup.get("process_exited") is True
                and cleanup.get("temporary_directory_removed") is True):
            # GDI instrumentation can change batching. Retry only for evidence
            # after cleanup; its result can never accept the original failure.
            original["diagnostic_retries"] = []
            for mode, staged in (("rapid", False), ("staged", True)):
                diagnostic = args.output.with_suffix(f".resize-{mode}-diagnostic.json")
                attempt = {"report": diagnostic.name, "mode": mode,
                           "purpose": "diagnostic only; original gate remains failed"}
                original["diagnostic_retries"].append(attempt)
                try:
                    windows_native.run(args.binary, diagnostic, args.arch,
                                       trace_runtime=True, staged_resize=staged)
                except BaseException:
                    traceback.print_exc()
                try:
                    evidence = json.loads(diagnostic.read_text(encoding="utf-8"))
                    cleanup = evidence.get("cleanup", {})
                    attempt["passed"] = evidence.get("passed") is True
                    attempt["cleanup_complete"] = (cleanup.get("process_exited") is True and
                                                   cleanup.get("temporary_directory_removed") is True)
                except (OSError, ValueError) as error:
                    attempt["error"] = repr(error)
                    attempt["cleanup_complete"] = False
                write_json(args.output, original)
                if not attempt["cleanup_complete"]:
                    break
        return 1
    # Preserve the original thirty checks in their original report. The second
    # suite has its own JSON/PNG evidence and must also pass on this desktop.
    import windows_workspace_native
    original = json.loads(args.output.read_text(encoding="utf-8"))
    workspace_path = args.output.with_suffix(".workspace.json")
    try:
        windows_workspace_native.run(args.binary, workspace_path, args.arch)
        workspace = json.loads(workspace_path.read_text(encoding="utf-8"))
    except BaseException as error:
        traceback.print_exc()
        try:
            workspace = json.loads(workspace_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            workspace = {"checks": []}
        workspace.update({"passed": False, "status": "failed"})
        workspace.setdefault("error", repr(error))
        write_json(workspace_path, workspace)
    result = windows_workspace_native.merge_reports(original, workspace)
    write_json(args.output, result)
    return 0 if result["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--arch", choices=["x64", "arm64"], required=True)
    parser.add_argument("--isolated-desktop", action="store_true")
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--worker-desktop", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    desktop_path = args.output.with_suffix(".desktop.json")
    state_path = args.output.with_suffix(".desktop.worker.json")
    try:
        require_ci(args.isolated_desktop)
        if not 0 < args.timeout <= 180:
            raise ValueError("worker timeout must be in (0, 180] seconds")
        if args.worker_desktop:
            if not re.fullmatch(r"ztty-native-[0-9]+-[0-9a-f]{32}", args.worker_desktop):
                raise DesktopBlocked("invalid owned desktop name")
            return worker(args, state_path)
        name = f"ztty-native-{os.getpid()}-{uuid.uuid4().hex}"
        command = [sys.executable, str(Path(__file__).resolve()), "--isolated-desktop",
                   "--worker-desktop", name, "--binary", str(args.binary),
                   "--output", str(args.output), "--arch", args.arch]
        write_json(args.output, {"passed": False, "status": "not_run", "checks": []})
        write_json(state_path, {"passed": False, "status": "not_run", "desktop": name})
        stdout = args.output.with_suffix(".desktop.stdout.log")
        stderr = args.output.with_suffix(".desktop.stderr.log")
        report = run_isolated(WindowsAPI(), command, name, stdout, stderr, args.timeout,
                              checkpoint=lambda value: write_json(desktop_path, value))
        report.update({"worker_state_file": state_path.name, "stdout": stdout.name, "stderr": stderr.name,
                       "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                       "source_sha": os.environ.get("SOURCE_SHA"), "report_file": desktop_path.name})
        try:
            native = json.loads(args.output.read_text(encoding="utf-8"))
            state = json.loads(state_path.read_text(encoding="utf-8"))
            if not isinstance(native, dict) or not isinstance(state, dict):
                raise ValueError("native/worker reports must be JSON objects")
            report["worker_state"] = state
            result = combine(native, report, state)
        except Exception as error:
            result = {"passed": False, "status": "failed", "error": repr(error), "desktop_isolation": report}
        write_json(desktop_path, report)
        write_json(args.output, result)
        print(json.dumps(result, indent=2))
        return 0 if result["passed"] else (2 if result["status"] == "blocked" else 1)
    except BaseException as error:
        report = {"passed": False, "status": "blocked" if isinstance(error, (DesktopBlocked, OSError)) else "failed",
                  "error": repr(error), "checks": []}
        write_json(args.output, report)
        # An early worker failure must not overwrite the controller checkpoint.
        write_json(state_path if args.worker_desktop else desktop_path, report)
        traceback.print_exc()
        return 2 if report["status"] == "blocked" else 1


if __name__ == "__main__":
    raise SystemExit(main())
