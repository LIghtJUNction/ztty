#!/usr/bin/env python3
"""Accept a real authenticated ssmcp --tty connection in ztty observer windows.

Start this coordinator, discover its unique client_name through ssmcp, and run
the commands from ready.json / await-second.json / await-third.json with native
exec. Record the bounded receipts described in docs/ssmcp-observer.md. No model
agent, mock relay, fake terminal, or account provisioning is involved.
"""

import argparse
import array
import ctypes
import fcntl
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import termios
import time
import tty
import uuid


JOB = r'''import json,os,sys,time
from pathlib import Path
root=Path(sys.argv[1]);phase=sys.argv[2]
with (root/'executions.jsonl').open('a') as output:
    output.write(json.dumps({'phase':phase,'pid':os.getpid(),'stdout_isatty':os.isatty(1),'stderr_isatty':os.isatty(2)})+'\n')
print('\x1b]2;ssmcp observer acceptance\x07\x1b[?1000h\x1b[?1006h',end='',flush=True)
print('\x1b[32mSSMCP_OBSERVER_'+phase.upper()+'\x1b[0m',flush=True)
print('Actual command PID: '+str(os.getpid()),flush=True)
print('stderr marker '+phase,file=sys.stderr,flush=True)
deadline=time.monotonic()+120
while not (root/('finish-'+phase)).exists():
    if time.monotonic()>=deadline: raise SystemExit('coordinator deadline')
    time.sleep(.025)
print('SSMCP_OBSERVER_'+phase.upper()+'_DONE',flush=True)
'''

CONFIG = """font = monospace:size=11
background = #000000
background-opacity = 100
background-blur = false
cursor-blink = false
window-padding = 12
"""


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def birth(pid):
    return Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]


def descendants():
    found = {}
    pending = [os.getpid()]
    while pending:
        parent = pending.pop()
        try:
            children = Path(f'/proc/{parent}/task/{parent}/children').read_text().split()
        except FileNotFoundError:
            continue
        for child in children:
            pid = int(child)
            try:
                fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
            except FileNotFoundError:
                continue
            if int(fields[1]) == parent and pid not in found:
                found[pid] = fields[19]
                pending.append(pid)
    return found


def signal_owned(pid, token, number):
    try:
        if birth(pid) == token:
            os.kill(pid, number)
    except (FileNotFoundError, ProcessLookupError):
        pass


def write_json(path, value):
    path = Path(path)
    pending = path.with_suffix(path.suffix + '.tmp')
    pending.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')
    pending.replace(path)


def main():
    from Xlib import X, XK, Xatom, display, protocol
    from Xlib.ext import xtest
    from PIL import Image

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--source-root', type=Path, required=True)
    parser.add_argument('--ssmcp', type=Path, required=True)
    parser.add_argument('--control-dir', type=Path, required=True,
                        help='new private directory used only by this run')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--screenshot', type=Path, required=True)
    parser.add_argument('--timeout', type=int, default=240)
    args = parser.parse_args()
    assert 30 <= args.timeout <= 600, 'timeout must be between 30 and 600 seconds'
    binary = args.binary.resolve(strict=True)
    source = args.source_root.resolve(strict=True)
    ssmcp = args.ssmcp.resolve(strict=True)
    wrapper = source / 'tools/ztty-observer'
    assert wrapper.is_file() and os.access(wrapper, os.X_OK), 'observer adapter must be executable'
    root = args.control_dir.resolve()
    root.mkdir(mode=0o700, parents=False, exist_ok=False)
    sys.path.insert(0, str(source / 'tools'))
    from isolated_xvfb import start_xvfb

    report = {
        'ok': False, 'platform': 'Linux X11 / owned Xvfb / actual authenticated ssmcp relay',
        'binary': str(binary), 'binary_sha256': digest(binary),
        'ssmcp_binary': str(ssmcp), 'ssmcp_sha256': digest(ssmcp),
        'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True, timeout=5).strip(),
        'source_changes': subprocess.check_output(['git', 'status', '--short'], cwd=source, text=True, timeout=5).splitlines(),
        'source_sha256': {name: digest(source / name) for name in (
            'src/main.rs', 'src/platform/linux.rs', 'src/session.rs', 'src/config_watch.rs',
            'tools/ztty-observer', 'tools/isolated_xvfb.py')},
        'test_sha256': digest(__file__), 'checks': [], 'receipts': {},
        'scope': 'two sequential jobs plus a third after closing the observer; no ssmcp model or agent tool calls',
    }
    connection = xvfb = client = None
    slave = None
    known_processes = {}
    deadline = time.monotonic() + args.timeout
    private = tempfile.TemporaryDirectory(prefix='private-', dir=root)
    private_root = Path(private.name)
    log = (private_root / 'client.log').open('wb')

    def interrupted(number, _frame):
        raise InterruptedError(f'test interrupted by signal {number}')

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError('PR_SET_CHILD_SUBREAPER failed')

    def wait(check, label, seconds=None):
        until = min(deadline, time.monotonic() + seconds) if seconds else deadline
        while time.monotonic() < until:
            if client is not None and client.poll() is not None:
                raise RuntimeError(f'ephemeral ssmcp client exited with status {client.returncode}; private account logs withheld')
            try:
                value = check()
                if value:
                    return value
            except (FileNotFoundError, ConnectionRefusedError):
                pass
            time.sleep(.02)
        raise TimeoutError(label)

    def receipt(name):
        path = root / name
        if not path.exists():
            return None
        assert path.stat().st_size <= 65536, 'oversized driver receipt'
        return json.loads(path.read_text())

    try:
        authorized = subprocess.run([str(ssmcp), 'whoami', '--json'], stdin=subprocess.DEVNULL,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        assert authorized.returncode == 0, 'existing ssmcp account authorization is unavailable; no login attempted'
        report['checks'].append('existing authenticated account reused without provisioning or reading credentials')
        xvfb = start_xvfb(screen='1500x1000x24', stderr=log)
        report['xvfb'] = xvfb.evidence
        connection = display.Display(xvfb.display)
        assert connection.has_extension('XTEST'), 'XTEST is required'
        screen = connection.screen()
        guard = screen.root.create_window(1250, 50, 150, 100, 0, screen.root_depth,
                                          X.InputOutput, X.CopyFromParent,
                                          background_pixel=0, event_mask=X.FocusChangeMask)
        guard.set_wm_name('Owned observer focus guard')
        guard.map()
        guard.set_input_focus(X.RevertToParent, X.CurrentTime)
        connection.sync()
        assert connection.get_input_focus().focus.id == guard.id
        config = private_root / 'config'
        config.write_text(CONFIG)
        # Keep ssmcp's authorized account configuration. Apply the private
        # appearance only to ztty through its documented executable adapter.
        shim = private_root / 'ztty-with-private-config'
        shim.write_text('#!' + sys.executable + '\nimport os,sys\nos.execv(' + repr(str(binary)) + ',[' +
                        repr(str(binary)) + ',"--config",' + repr(str(config)) + ']+sys.argv[1:])\n')
        shim.chmod(0o700)
        name = 'ztty-observer-acceptance-' + uuid.uuid4().hex[:12]
        env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS='@im=none', LC_ALL='C.UTF-8',
                   XDG_RUNTIME_DIR=str(private_root), ZTTY_BIN=str(shim),
                   ZTTY_SOCKET='@' + name, ZTTY_OBSERVER_TEST_RUN=name)
        env.pop('WAYLAND_DISPLAY', None)
        job_file = root / 'job.py'
        job_file.write_text(JOB)
        client = subprocess.Popen([str(ssmcp), '--name', name, '--title', 'ztty native observer acceptance',
                                   '--no-browser', '--no-setup', '--no-watchdog', '--no-color',
                                   '--shell', '/bin/sh', '--tty', str(wrapper)],
                                  cwd=root, env=env, stdin=subprocess.DEVNULL,
                                  stdout=log, stderr=log, start_new_session=True)
        known_processes[client.pid] = birth(client.pid)
        report['client_name'] = name
        report['client_pid'] = client.pid

        def command(phase):
            return ['/usr/bin/python3', '-u', str(job_file), str(root), phase]

        write_json(root / 'ready.json', {'client_name': name, 'client_pid': client.pid,
                                       'first_command': command('first'), 'receipt': 'first-start.json'})

        def rpc(request):
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                channel.settimeout(5)
                channel.connect('\0' + env['ZTTY_SOCKET'][1:])
                channel.sendall(json.dumps(request).encode() + b'\n')
                raw = bytearray()
                while b'\n' not in raw:
                    part = channel.recv(8192)
                    if not part:
                        raise EOFError('private broker disconnected')
                    raw.extend(part)
                    assert len(raw) <= 1048576, 'oversized broker response'
            value = json.loads(raw.split(b'\n', 1)[0])
            assert value['ok'], value
            return value['result']

        def sessions():
            return rpc({'op': 'list'})['sessions']

        first_start = wait(lambda: receipt('first-start.json'), 'driver must start first job through registered native exec')
        assert first_start['output_mode'] == 'pty', first_start
        report['receipts']['first_start'] = first_start
        first_sessions = wait(sessions, 'observer session appears', 20)
        assert len(first_sessions) == 1, first_sessions
        identifier = first_sessions[0]['uri']
        info = rpc({'op': 'info', 'session': identifier})
        assert info['read_only'] and info['attached'], info
        first_info = dict(info)
        report['first_session'] = info
        for pid in (info['pid'], info['broker_pid']):
            known_processes[pid] = birth(pid)
        wait(lambda: 'SSMCP_OBSERVER_FIRST' in rpc({'op': 'read', 'session': identifier})['screen'],
             'actual command output reaches observer', 15)
        assert connection.get_input_focus().focus.id == guard.id, 'observer stole focus from existing window'
        report['checks'].append('actual ssmcp PTY output reached a fresh read-only ztty window without stealing X11 focus')
        argv = Path(f"/proc/{info['pid']}/cmdline").read_bytes().split(b'\0')
        assert len(argv) >= 4 and argv[1] == b'__tty-view', 'ztty child is not the actual ssmcp viewer'
        assert Path(os.fsdecode(argv[0])).resolve() == ssmcp
        assert Path(os.fsdecode(argv[2])).name.startswith(f'ssmcp-tty-{client.pid}-'), 'viewer belongs to another client'
        assert int(argv[3]) == first_start['tty_slot'], 'viewer slot does not match native exec lease'
        report['viewer_slot'] = int(argv[3])
        report['checks'].append('ztty runs the installed ssmcp __tty-view for the exact native exec slot')

        view = connection.create_resource_object('window', info['window_id'])
        slave = os.open(f"/proc/{info['pid']}/fd/0", os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        old_termios = termios.tcgetattr(slave)
        tty.setraw(slave)

        def queued_input():
            count = array.array('i', [0])
            fcntl.ioctl(slave, termios.FIONREAD, count, True)
            return count[0]

        def key(*names):
            codes = [connection.keysym_to_keycode(XK.string_to_keysym(name)) for name in names]
            assert all(codes), names
            for code in codes:
                xtest.fake_input(connection, X.KeyPress, code)
            for code in reversed(codes):
                xtest.fake_input(connection, X.KeyRelease, code)
            connection.sync()

        clipboard = connection.intern_atom('CLIPBOARD')
        utf8 = connection.intern_atom('UTF8_STRING')
        targets = connection.intern_atom('TARGETS')
        guard.set_selection_owner(clipboard, X.CurrentTime)
        connection.sync()
        clipboard_requests = []

        def pump_clipboard():
            while connection.pending_events():
                event = connection.next_event()
                if event.type != X.SelectionRequest:
                    continue
                clipboard_requests.append(event.target)
                prop = event.property or event.target
                target = connection.create_resource_object('window', event.requestor.id)
                if event.target == targets:
                    target.change_property(prop, Xatom.ATOM, 32, [utf8, Xatom.STRING])
                elif event.target in (utf8, Xatom.STRING):
                    target.change_property(prop, event.target, 8, b'OBSERVER_CLIPBOARD_MUST_NOT_REACH_PTY\n')
                else:
                    prop = X.NONE
                target.send_event(protocol.event.SelectionNotify(time=event.time,
                    requestor=event.requestor, selection=event.selection, target=event.target, property=prop))
                connection.flush()

        view.set_input_focus(X.RevertToParent, X.CurrentTime)
        connection.sync()
        key('a')
        key('Return')
        key('Control_L', 'Shift_L', 'v')
        view.warp_pointer(30, 30)
        xtest.fake_input(connection, X.ButtonPress, 1)
        xtest.fake_input(connection, X.ButtonRelease, 1)
        connection.sync()
        until = time.monotonic() + .5
        while time.monotonic() < until:
            pump_clipboard()
            assert queued_input() == 0, 'observer injected bytes into its viewer PTY'
            time.sleep(.01)
        assert not clipboard_requests, 'read-only observer requested clipboard data'
        report['checks'].append('XTEST keyboard, paste shortcut and mouse events neither queued PTY input nor requested clipboard data')

        before_size = termios.tcgetwinsize(slave)
        geometry = view.get_geometry()
        view.configure(width=geometry.width + 80, height=geometry.height + 40)
        connection.sync()
        wait(lambda: view.get_geometry().width == geometry.width + 80, 'observer resize reaches X11', 5)
        time.sleep(.15)
        assert termios.tcgetwinsize(slave) == before_size, 'observer resize changed viewer PTY size'
        assert rpc({'op': 'info', 'session': identifier})['read_only']
        report['checks'].append('resizing the read-only observation window leaves its viewer PTY dimensions unchanged')
        view.configure(width=geometry.width, height=geometry.height)
        guard.set_input_focus(X.RevertToParent, X.CurrentTime)
        connection.sync()
        termios.tcsetattr(slave, termios.TCSANOW, old_termios)

        (root / 'finish-first').touch()
        write_json(root / 'await-second.json', {'first_job_id': first_start['job_id'],
                                              'second_command': command('second'),
                                              'receipt': 'second-start.json'})
        first_done = wait(lambda: receipt('first-done.json'), 'driver must record completed first native exec')
        assert first_done['exit_code'] == 0 and not first_done['running'], first_done
        assert 'SSMCP_OBSERVER_FIRST_DONE' in first_done['stdout'] and 'stderr marker first' in first_done['stdout']
        assert first_done['stderr'] == '', 'PTY output should have a single actual stream'
        report['receipts']['first_done'] = first_done
        second_start = wait(lambda: receipt('second-start.json'), 'driver must start second native exec')
        assert second_start['output_mode'] == 'pty' and second_start['tty_slot'] == first_start['tty_slot'], second_start
        report['receipts']['second_start'] = second_start
        wait(lambda: 'SSMCP_OBSERVER_SECOND' in rpc({'op': 'read', 'session': identifier})['screen'],
             'second actual command output reaches reused observer', 15)
        second_sessions = sessions()
        assert len(second_sessions) == 1, second_sessions
        second_info = rpc({'op': 'info', 'session': identifier})
        assert (second_info['id'], second_info['pid'], second_info['window_id']) == (
            first_info['id'], first_info['pid'], first_info['window_id']), 'sequential command created another viewer or GUI'
        assert second_info['read_only'] and connection.get_input_focus().focus.id == guard.id
        report['second_session'] = second_info
        report['checks'].append('sequential native exec reuses the same ssmcp slot, viewer process and ztty window without focus theft')
        executions = [json.loads(line) for line in (root / 'executions.jsonl').read_text().splitlines()]
        assert [value['phase'] for value in executions] == ['first', 'second'], executions
        assert all(value['stdout_isatty'] and value['stderr_isatty'] for value in executions), executions
        assert executions[0]['pid'] != executions[1]['pid'], executions
        report['executions'] = executions
        report['checks'].append('two commands executed exactly once each with real PTY stdout/stderr; the observer did not rerun either command')

        def rendered(target=view):
            size = target.get_geometry()
            pixels = target.get_image(0, 0, size.width, size.height, X.ZPixmap, 0xFFFFFFFF)
            image = Image.frombytes('RGB', (size.width, size.height), pixels.data, 'raw', 'BGRX')
            bright = sum(1 for pixel in image.getdata() if max(pixel) > 96)
            return image if bright > 500 else None

        screenshot = wait(rendered, 'observer output must have rendered visible glyphs', 5)
        args.screenshot.parent.mkdir(parents=True, exist_ok=True)
        screenshot.save(args.screenshot)
        report['screenshot'] = {'path': str(args.screenshot), 'sha256': digest(args.screenshot),
                                'width': screenshot.width, 'height': screenshot.height}
        (root / 'finish-second').touch()
        write_json(root / 'await-finish.json', {'second_job_id': second_start['job_id'], 'receipt': 'second-done.json'})
        second_done = wait(lambda: receipt('second-done.json'), 'driver must record completed second native exec')
        assert second_done['exit_code'] == 0 and not second_done['running'], second_done
        assert 'SSMCP_OBSERVER_SECOND_DONE' in second_done['stdout'] and 'stderr marker second' in second_done['stdout']
        assert second_done['stderr'] == ''
        report['receipts']['second_done'] = second_done
        report['checks'].append('both real native exec jobs completed with exit 0 and their exact combined PTY streams were retained')
        os.close(slave)
        slave = None

        protocols = connection.intern_atom('WM_PROTOCOLS')
        delete = connection.intern_atom('WM_DELETE_WINDOW')
        view.send_event(protocol.event.ClientMessage(window=view, client_type=protocols,
                        data=(32, [delete, X.CurrentTime, 0, 0, 0])), event_mask=0)
        connection.sync()
        wait(lambda: not any(item['attached'] for item in sessions()), 'observer window closes through WM_DELETE_WINDOW', 5)
        report['closed_window_sessions'] = sessions()
        write_json(root / 'await-third.json', {'third_command': command('third'), 'receipt': 'third-start.json'})
        third_start = wait(lambda: receipt('third-start.json'), 'driver must start third native exec after closing observer')
        assert third_start['output_mode'] == 'pty', third_start
        report['receipts']['third_start'] = third_start
        visible = None
        visible_until = min(deadline, time.monotonic() + 12)
        while time.monotonic() < visible_until:
            for item in sessions():
                if item['attached'] and 'SSMCP_OBSERVER_THIRD' in rpc({'op': 'read', 'session': item['uri']})['screen']:
                    visible = rpc({'op': 'info', 'session': item['uri']})
                    break
            if visible:
                break
            time.sleep(.05)
        report['next_command_after_close'] = {'visible': visible is not None, 'session': visible,
                                             'sessions': sessions()}
        if visible is not None:
            reopened_window = connection.create_resource_object('window', visible['window_id'])
            reopened = wait(lambda: rendered(reopened_window), 'reopened observer renders the third command', 5)
            reopened_path = args.screenshot.with_name(args.screenshot.stem + '-reopened' + args.screenshot.suffix)
            reopened.save(reopened_path)
            report['reopened_screenshot'] = {'path': str(reopened_path), 'sha256': digest(reopened_path),
                                             'width': reopened.width, 'height': reopened.height}
        (root / 'finish-third').touch()
        write_json(root / 'await-third-finish.json', {'third_job_id': third_start['job_id'], 'receipt': 'third-done.json'})
        third_done = wait(lambda: receipt('third-done.json'), 'driver must record completed third native exec')
        assert third_done['exit_code'] == 0 and not third_done['running'], third_done
        assert 'SSMCP_OBSERVER_THIRD_DONE' in third_done['stdout']
        report['receipts']['third_done'] = third_done
        report['executions'] = [json.loads(line) for line in (root / 'executions.jsonl').read_text().splitlines()]
        assert [value['phase'] for value in report['executions']] == ['first', 'second', 'third']
        assert visible is not None, 'next real command completed, but closing the observer left no visible window'
        assert visible['read_only'] and connection.get_input_focus().focus.id == guard.id
        assert visible['pid'] != first_info['pid'], 'closed observer retained its original viewer process'
        assert (visible['broker_pid'], visible['created_at'], visible['id']) != (
            first_info['broker_pid'], first_info['created_at'], first_info['id']), 'closed observer session was not retired'
        # X11 resource IDs may be recycled when a display connection closes.
        # The broker/session incarnation above identifies the new window.
        report['next_command_after_close']['new_session_incarnation'] = True
        report['checks'].append('the next command after WM_DELETE_WINDOW obtains a visible read-only observer without stealing focus')
        for item in sessions():
            subprocess.run([str(binary), 'close', item['uri']], env=env, check=True,
                           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=log, timeout=5)
        wait(lambda: not sessions(), 'explicit close retires the private viewer session', 5)
        report['checks'].append('explicit ztty close retires the private observer session after commands finish')
        report['ok'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        # Account connection logs may contain authorization links on failure.
        # They remain private and are deleted, never copied into public evidence.
        if slave is not None:
            os.close(slave)
        if client is not None and client.poll() is None:
            signal_owned(client.pid, known_processes[client.pid], signal.SIGTERM)
            try:
                client.wait(timeout=5)
            except subprocess.TimeoutExpired:
                signal_owned(client.pid, known_processes[client.pid], signal.SIGKILL)
                client.wait(timeout=3)
        if connection is not None:
            connection.close()
        for pid, token in known_processes.items():
            signal_owned(pid, token, signal.SIGTERM)
        if xvfb is not None:
            xvfb.close()
        until = time.monotonic() + 3
        while time.monotonic() < until:
            for pid, token in descendants().items():
                signal_owned(pid, token, signal.SIGKILL)
            try:
                pid, _ = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                break
            if pid == 0:
                time.sleep(.02)
        report['cleanup'] = {'owned_processes_remaining': list(descendants()),
                             'account_logs_published': False}
        if report['cleanup']['owned_processes_remaining']:
            report['ok'] = False
        log.close()
        private.cleanup()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        write_json(args.output, report)
        write_json(root / 'finished.json', {'ok': report['ok'], 'report': str(args.output)})
    print(json.dumps({'ok': report['ok'], 'checks': report['checks'], 'error': report.get('error'),
                      'report': str(args.output)}, ensure_ascii=False, indent=2))
    return 0 if report['ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
