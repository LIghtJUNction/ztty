#!/usr/bin/env python3
"""Profile owned ztty window memory in seven stages, without latency benchmarks."""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import select
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import traceback

GEOMETRY = (120, 32)
FONT = 'DejaVu Sans Mono:size=13'
REQUIRED = ('Rss_bytes', 'Pss_bytes', 'Private_Clean_bytes', 'Private_Dirty_bytes')
COUNTERS = REQUIRED + ('Pss_Anon_bytes', 'Pss_File_bytes', 'Pss_Shmem_bytes',
                       'Shared_Clean_bytes', 'Shared_Dirty_bytes', 'Swap_bytes')


def receive(connection, limit=65536):
    data = bytearray()
    while b'\n' not in data:
        block = connection.recv(8192)
        if not block:
            raise EOFError('control channel closed')
        data.extend(block)
        if len(data) > limit:
            raise RuntimeError('oversized control response')
    return json.loads(data.split(b'\n', 1)[0])


def exchange(address, request):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
        channel.settimeout(15)
        channel.connect(address)
        channel.sendall(json.dumps(request).encode() + b'\n')
        return receive(channel)


def write_all(data):
    view = memoryview(data)
    while view:
        count = os.write(1, view)
        if count <= 0:
            raise EOFError('PTY output closed')
        view = view[count:]


def child(control_path):
    import tty
    tty.setraw(0)
    write_all(b'\x1b[?25l')
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(control_path)
        listener.listen(1)
        while True:
            ready, _, _ = select.select([0, listener], [], [])
            if 0 in ready:
                try:
                    if not os.read(0, 8192):
                        return
                except OSError:
                    return
            if listener not in ready:
                continue
            connection, _ = listener.accept()
            with connection:
                connection.settimeout(15)
                request = receive(connection)
                action = request.get('op')
                emitted = 0
                if action in ('short', 'wide'):
                    width = 5 if action == 'short' else 117
                    # 35,000 / 595,000 bytes total, never one large allocation.
                    for start in range(0, 5000, 64):
                        payload = b''.join((f'{index:04d}x'.ljust(width, 'x') + '\r\n').encode()
                                           for index in range(start, min(start + 64, 5000)))
                        write_all(payload)
                        emitted += len(payload)
                elif action == 'clear':
                    write_all(b'\x1b[H\x1b[2J')
                elif action == 'clear_history':
                    write_all(b'\x1b[3J\x1b[H\x1b[2J')
                elif action != 'ready':
                    raise ValueError('unknown owned probe command')
                write_all(b'\x1b[6n')
                answer = bytearray()
                deadline = time.monotonic() + 15
                position = None
                while time.monotonic() < deadline:
                    remaining = max(0, deadline - time.monotonic())
                    if not select.select([0], [], [], remaining)[0]:
                        break
                    block = os.read(0, 8192)
                    if not block:
                        raise EOFError('PTY closed before DSR')
                    answer.extend(block)
                    position = re.search(rb'\x1b\[([0-9]+);([0-9]+)R', answer)
                    if position:
                        break
                if position is None:
                    raise TimeoutError('DSR completion barrier')
                size = os.get_terminal_size(0)
                connection.sendall(json.dumps({'ok': True, 'pid': os.getpid(),
                    'cols': size.columns, 'rows': size.lines,
                    'cursor': [int(position[1]), int(position[2])],
                    'emitted_bytes': emitted}).encode() + b'\n')


def wait(predicate, label, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (FileNotFoundError, ConnectionRefusedError):
            pass
        time.sleep(0.02)
    raise TimeoutError(label)


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def aggregate(rows):
    pids = [row['pid'] for row in rows]
    if len(pids) != len(set(pids)):
        raise RuntimeError('duplicate process in memory total')
    for row in rows:
        if any(key not in row for key in REQUIRED):
            raise RuntimeError('required process memory field is unavailable')
    values = {key: sum(row[key] for row in rows) if all(key in row for row in rows) else None
              for key in COUNTERS}
    values['Private_bytes'] = values['Private_Clean_bytes'] + values['Private_Dirty_bytes']
    return values


def pixmap_geometry(connection, owner_window):
    """Optional XRes 1.2 detail for this owned X client, excluding crossrefs."""
    result = {'available': False, 'client_window': owner_window,
              'scope': 'Owned client pixmap XIDs only; zero-XID internal resources and cross references are not summed.'}
    try:
        version = connection.res_query_version()
        result['extension_version'] = [int(version.server_major), int(version.server_minor)]
        if tuple(result['extension_version']) < (1, 2):
            raise RuntimeError('XRes 1.2 resource byte query is unavailable')
        pixmap_type = connection.intern_atom('PIXMAP')
        values = connection.res_query_resource_bytes(client=owner_window,
                    specs=[{'resource': 0, 'type': pixmap_type}]).sizes
        rows = []
        internal_count = 0
        for value in values[:512]:
            size = value.size
            if not size.resource:
                internal_count += 1
                continue
            row = {key: int(getattr(size, key)) for key in
                   ('resource', 'type', 'bytes', 'ref_count', 'use_count')}
            if size.type == pixmap_type:
                try:
                    geometry = connection.create_resource_object('pixmap', size.resource).get_geometry()
                    row.update(width=int(geometry.width), height=int(geometry.height), depth=int(geometry.depth))
                except Exception as error:
                    row['geometry_error'] = repr(error)
            rows.append(row)
        result.update(available=True, resources=rows, internal_resource_count=internal_count,
                      truncated=len(values) > 512)
    except Exception as error:
        result['error'] = repr(error)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--renderer', choices=('software', 'gpu'), required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    repo = args.repo.resolve(strict=True)
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    if output.exists():
        raise FileExistsError('Output already exists; use a new path')
    sys.path.insert(0, str(repo / 'tools'))
    from measure_runtime import memory, process_stat, descendants, same_process, pixmap_bytes
    from measure_multiwindow import owned_processes, terminal_windows, cleanup
    from isolated_xvfb import start_xvfb
    from gui_renderer import record_renderer
    from Xlib import display
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError('PR_SET_CHILD_SUBREAPER failed')
    binary_sha = digest(binary)
    report = {'schema': 1, 'ok': False, 'binary': str(binary), 'binary_sha256': binary_sha,
        'version': subprocess.check_output([str(binary), '--version'], text=True, timeout=5).strip(),
        'source_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo,
                                             text=True, timeout=5).strip(),
        'source_hashes': {name: digest(repo / 'tools' / name) for name in
                         ('measure_runtime.py', 'measure_multiwindow.py', 'isolated_xvfb.py', 'gui_renderer.py')},
        'collector_sha256': digest(__file__),
        'settings': {'font': FONT, 'cols': 120, 'rows': 32, 'history': 2000,
                     'renderer': args.renderer, 'LIBGL_ALWAYS_SOFTWARE': '1', 'LP_NUM_THREADS': '2',
                     'settle_seconds': 1.0, 'samples_per_stage': 3, 'sample_interval_seconds': 0.1},
        'scope': 'Owned shared broker and raw PTYs, private Xvfb; GPU means software OpenGL here, not hardware VRAM.',
        'accounting': 'Broker is included once in application totals. PTY children, Xvfb and XRes are separate; do not add RSS to PSS or XRes bytes to Xvfb memory.',
        'stages': []}
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('x') as stream:
        json.dump(report, stream)

    def save():
        temporary_output = output.with_name(output.name + '.tmp')
        temporary_output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        os.replace(temporary_output, output)

    server = connection = None
    launchers = []
    owned_ids = {}
    names = []
    controls = {}
    with tempfile.TemporaryDirectory(prefix='ztty-memory-stages-') as temporary:
        directory = Path(temporary)
        with (directory / 'terminal.log').open('wb') as terminal_log, (directory / 'xvfb.log').open('wb') as xlog:
            try:
                server = start_xvfb(screen='7680x2160x24', dpi=96, stderr=xlog)
                connection = display.Display(server.display)
                report['display_ownership'] = server.evidence
                report['xserver_before'] = memory(server.process.pid)
                env = dict(os.environ, DISPLAY=server.display, XMODIFIERS='@im=none', LC_ALL='C.UTF-8',
                    LIBGL_ALWAYS_SOFTWARE='1', LP_NUM_THREADS='2', ZTTY_MEMORY_TAG=temporary,
                    ZTTY_SOCKET='@ztty-memory-stages-' + directory.name,
                    XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(directory / 'config-home'),
                    XDG_CACHE_HOME=str(directory / 'cache'), XDG_STATE_HOME=str(directory / 'state'))
                env.pop('WAYLAND_DISPLAY', None)
                env.pop('LD_PRELOAD', None)
                address = '\0' + env['ZTTY_SOCKET'][1:]
                config = directory / 'ztty.conf'
                config.write_text(f'font = {FONT}\nrenderer = {args.renderer}\nwindow-padding = 0\n'
                    'background-opacity = 100\nbackground-blur = false\ncursor-blink = false\n')

                def rpc(request):
                    response = exchange(address, request)
                    if not response.get('ok'):
                        raise RuntimeError('Owned broker request failed: ' + json.dumps(response))
                    return response['result']

                def infos():
                    values = [rpc({'op': 'info', 'session': name}) for name in names]
                    brokers = {value['broker_pid'] for value in values}
                    if len(brokers) != 1:
                        raise RuntimeError('All windows must share exactly one broker')
                    for value in values:
                        if (value['cols'], value['rows']) != GEOMETRY or not value['running']:
                            raise RuntimeError('Owned PTY is not running at the fixed geometry')
                    return values

                def control(name, action):
                    value = exchange(str(controls[name]), {'op': action})
                    if not value.get('ok') or (value['cols'], value['rows']) != GEOMETRY:
                        raise RuntimeError('Owned raw PTY control failed')
                    return value

                def screen(name, expected):
                    value = rpc({'op': 'read', 'session': name, 'max_bytes': 1})['screen']
                    valid = not value.strip() if expected == '' else expected in value
                    return {'sha256': hashlib.sha256(value.encode()).hexdigest(),
                            'nonempty_rows': sum(bool(line.strip()) for line in value.splitlines())} if valid else None

                def spawn(index):
                    name = f'memory-{index}'
                    controls[name] = directory / f'control-{index}.sock'
                    command = [str(binary), '--name', name, '--config', str(config), '--backend', 'x11',
                        '--geometry', '120x32', '--scrollback', '2000', '-e', sys.executable,
                        str(Path(__file__).resolve()), '--child', str(controls[name])]
                    process = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                                               stderr=terminal_log, start_new_session=True)
                    launchers.append(process)
                    with contextlib.suppress(OSError):
                        owned_ids[process.pid] = process_stat(process.pid)['start_ticks']
                    wait(controls[name].exists, 'raw PTY control socket')
                    names.append(name)
                    current = infos()
                    value = next(item for item in current if item['name'] == name)
                    owned_ids[value['pid']] = process_stat(value['pid'])['start_ticks']
                    owned_ids[value['broker_pid']] = process_stat(value['broker_pid'])['start_ticks']
                    ready = control(name, 'ready')
                    if ready['pid'] != value['pid']:
                        raise RuntimeError('Control PID differs from broker PTY owner')
                    view = connection.create_resource_object('window', value['window_id'])
                    geometry = view.get_geometry()
                    x, y = (index % 5) * 1536, (index // 5) * 1080
                    if geometry.width > 1536 or geometry.height > 1080 or x + geometry.width > 7680 or y + geometry.height > 2160:
                        raise RuntimeError('Window does not fit the fixed nonoverlapping display layout')
                    view.configure(x=x, y=y)
                    connection.sync()
                    wait(lambda: screen(name, ''), 'blank initial screen')

                def process_sample(pid, started):
                    if not same_process(pid, started):
                        raise RuntimeError('Owned process identity changed before memory sampling')
                    info = process_stat(pid)
                    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
                    value = {**info, 'ppid': int(fields[1]), 'session_id': int(fields[3]),
                             'tty_nr': int(fields[4]), **memory(pid)}
                    with contextlib.suppress(OSError):
                        value['stdin'] = os.readlink(f'/proc/{pid}/fd/0')
                    if not same_process(pid, started):
                        raise RuntimeError('Owned process identity changed while sampling')
                    return value

                def sample(current):
                    broker_pid = current[0]['broker_pid']
                    terminal_ids = owned_processes(binary, temporary)
                    if broker_pid not in terminal_ids:
                        raise RuntimeError('Broker is missing from uniquely tagged terminal ownership')
                    if Path(f'/proc/{broker_pid}/exe').resolve() != binary:
                        raise RuntimeError('Broker executable differs from requested binary')
                    child_ids = {}
                    for value in current:
                        pid = value['pid']
                        child_ids[pid] = process_stat(pid)['start_ticks']
                        child_ids.update(descendants(pid))
                    all_descendants = {}
                    for pid in terminal_ids:
                        all_descendants.update(descendants(pid))
                    app_ids = {**terminal_ids, **{pid: started for pid, started in all_descendants.items()
                                                if pid not in child_ids}}
                    owned_ids.update(app_ids)
                    owned_ids.update(child_ids)
                    application = [process_sample(pid, started) for pid, started in sorted(app_ids.items())]
                    children = [process_sample(pid, started) for pid, started in sorted(child_ids.items())]
                    broker = next(item for item in application if item['pid'] == broker_pid)
                    for value in current:
                        child_row = next(item for item in children if item['pid'] == value['pid'])
                        if child_row['ppid'] != broker_pid:
                            raise RuntimeError('PTY child parent differs from the owned broker')
                    xserver = memory(server.process.pid)
                    # One call per X client/broker, not one repeated total per window.
                    pixmap = pixmap_bytes(server.display, current[0]['window_id'])
                    return {'sampled_monotonic': time.monotonic(), 'broker': broker,
                            'application': aggregate(application), 'application_processes': application,
                            'pty_children': children, 'pty_children_total': aggregate(children),
                            'xserver': xserver, 'xres_pixmap': pixmap}

                def stage(label, barriers):
                    time.sleep(1.0)
                    current = infos()
                    observed = terminal_windows(connection)
                    if len(observed) != len(names) or set(observed) != {value['window_id'] for value in current}:
                        raise RuntimeError('Observed windows differ from the owned session list')
                    if {row['pid'] for row in observed.values()} != {current[0]['broker_pid']}:
                        raise RuntimeError('Native window owners differ from the shared broker')
                    renderer_rows = [record_renderer(connection, row['window'], args.renderer)
                                     for row in observed.values()]
                    geometries = []
                    for window_id, value in observed.items():
                        shape = value['window'].get_geometry()
                        geometries.append({'window_id': window_id, 'width': shape.width, 'height': shape.height})
                    snapshots = []
                    for index in range(3):
                        snapshots.append(sample(current))
                        if index != 2:
                            time.sleep(0.1)
                    for group in ('application_processes', 'pty_children'):
                        identities = [{(row['pid'], row['start_ticks']) for row in snap[group]} for snap in snapshots]
                        if any(value != identities[0] for value in identities[1:]):
                            raise RuntimeError('Process set changed within a memory plateau sample')
                    medians = {}
                    for group in ('broker', 'application', 'pty_children_total', 'xserver'):
                        medians[group] = {key: statistics.median(snap[group][key] for snap in snapshots)
                                          if all(snap[group].get(key) is not None for snap in snapshots) else None
                                          for key in COUNTERS}
                    pixmaps = [snap['xres_pixmap'].get('bytes') for snap in snapshots]
                    medians['xres_pixmap_bytes'] = statistics.median(pixmaps) if all(value is not None for value in pixmaps) else None
                    row = {'stage': label, 'window_count': len(names), 'sessions': current,
                           'barriers': barriers, 'renderers': renderer_rows, 'window_geometry': geometries,
                           'xres_owned_pixmap_geometry': pixmap_geometry(connection, current[0]['window_id']),
                           'one_32bit_surface_per_window_estimate_bytes': sum(g['width'] * g['height'] * 4 for g in geometries),
                           'samples': snapshots, 'median': medians}
                    report['stages'].append(row)
                    save()
                    print(json.dumps({'stage': label, 'windows': len(names),
                        'broker_pss_bytes': medians['broker']['Pss_bytes'],
                        'broker_rss_bytes': medians['broker']['Rss_bytes'],
                        'broker_private_dirty_bytes': medians['broker']['Private_Dirty_bytes'],
                        'application_pss_bytes': medians['application']['Pss_bytes'],
                        'children_pss_bytes': medians['pty_children_total']['Pss_bytes'],
                        'xserver_pss_bytes': medians['xserver']['Pss_bytes'],
                        'xserver_rss_bytes': medians['xserver']['Rss_bytes'],
                        'xserver_private_dirty_bytes': medians['xserver']['Private_Dirty_bytes'],
                        'xres_pixmap_bytes': medians['xres_pixmap_bytes']}), flush=True)

                spawn(0)
                stage('one_blank', [control(names[0], 'ready')])
                for index in range(1, 10):
                    spawn(index)
                stage('ten_blank', [control(name, 'ready') for name in names])
                target = names[-1]
                for action in ('short', 'wide'):
                    barrier = control(target, action)
                    expected = '4999x' if action == 'short' else '4999x' + 'x' * 112
                    barrier['screen'] = wait(lambda: screen(target, expected), action + ' output screen')
                    stage(action + '_burst', [barrier])
                for action, label in [('clear', 'screen_cleared_history_retained'),
                                      ('clear_history', 'screen_and_history_cleared')]:
                    barrier = control(target, action)
                    barrier['screen'] = wait(lambda: screen(target, ''), 'cleared screen')
                    stage(label, [barrier])
                removed = names[1:]
                removed_pids = {value['pid']: process_stat(value['pid'])['start_ticks']
                                for value in infos() if value['name'] in removed}
                for name in reversed(removed):
                    rpc({'op': 'close', 'session': name})
                names[:] = names[:1]
                wait(lambda: len(rpc({'op': 'list'})['sessions']) == 1, 'nine sessions closed')
                wait(lambda: not any(same_process(pid, started) for pid, started in removed_pids.items()),
                     'nine closed PTY children reaped')
                stage('nine_closed_one_blank_remains', [control(names[0], 'ready')])
                if digest(binary) != binary_sha:
                    raise RuntimeError('Binary changed during the profile')
                report['ok'] = True
            except BaseException as error:
                report['error'] = repr(error)
                report['traceback'] = traceback.format_exc()
            finally:
                owned_ids.update(descendants(os.getpid()))
                if 'rpc' in locals():
                    for name in reversed(names):
                        with contextlib.suppress(Exception):
                            rpc({'op': 'close', 'session': name})
                for pid, started in owned_processes(binary, temporary).items():
                    owned_ids[pid] = started
                    owned_ids.update(descendants(pid))
                if server is not None:
                    owned_ids.pop(server.process.pid, None)
                remaining = cleanup(owned_ids)
                for process in launchers:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=1)
                if connection is not None:
                    connection.close()
                if server is not None:
                    server.close()
                    report['xserver_cleanup_exit_code'] = server.process.returncode
                report['remaining_owned_pids'] = remaining
                if remaining:
                    report['ok'] = False
                terminal_log.flush()
                xlog.flush()
                report['terminal_stderr_tail'] = (directory / 'terminal.log').read_text(errors='replace')[-8000:]
                report['xvfb_stderr_tail'] = (directory / 'xvfb.log').read_text(errors='replace')[-2000:]
                save()
    print(json.dumps({'ok': report['ok'], 'stages': len(report['stages']),
                      'remaining_owned_pids': report['remaining_owned_pids'],
                      'error': report.get('error'), 'output': str(output)}), flush=True)
    return 0 if report['ok'] else 1


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--child':
        child(sys.argv[2])
    else:
        raise SystemExit(main())
