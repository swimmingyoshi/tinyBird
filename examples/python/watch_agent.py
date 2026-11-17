"""Play a trusted local training run in a visible desktop window."""
import argparse
import traceback
import time
import tkinter as tk
from tkinter import ttk
from pathlib import Path

from PIL import Image, ImageTk
import torch
import numpy as np
from tinybird_env import TinyBirdEnv
from train import load_run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path, help='Local run directory containing manifest.json and model.zip')
    parser.add_argument('--checkpoint', choices=['selected', 'best', 'final'], default='selected',
                        help='Inspect the selected model, best newly trained checkpoint, or final weights')
    parser.add_argument('--episodes', type=int, default=0, help='Exit after this many episodes; zero loops until closed')
    parser.add_argument('--start-variant', type=int, help='Always start at a particular configured checkpoint (zero-based)')
    parser.add_argument('--agent-view', action='store_true', help='Show the actual visual frame stack below the game')
    parser.add_argument('--finish-animation-frames', type=int, default=0, help='Show automatic celebration and score tally after success (0–600 frames)')
    args = parser.parse_args()
    if args.episodes < 0:
        parser.error('episodes must be nonnegative')
    if not 0 <= args.finish_animation_frames <= 600:
        parser.error('finish-animation-frames must be 0–600')
    torch.set_num_threads(1)
    manifest, inputs, model = load_run(args.run)
    if args.checkpoint != 'selected':
        checkpoint = args.run / ('best/best_model.zip' if args.checkpoint == 'best' else 'final_model.zip')
        if not checkpoint.is_file():
            parser.error(f'Checkpoint does not exist: {checkpoint}')
        model = type(model).load(checkpoint, device='cpu')
    if args.start_variant is not None and not 0 <= args.start_variant < len(inputs['task'].get('reset_variants', [])):
        parser.error('start-variant is outside the configured checkpoints')
    env = TinyBirdEnv(**inputs, render_mode='rgb_array')
    root = None
    try:
        root = tk.Tk()
        root.title(f'tinyBird — AI playback — {args.run.name} ({args.checkpoint})')
        root.configure(bg='#111c27')
        hybrid = bool(env.task.get('visual'))
        tk.Label(root, text=('Visual + RAM' if hybrid else 'RAM') + ' policy · Playback (learning is off)',
                 bg='#111c27', fg='#adc3d6', font=('Segoe UI',10)).pack(pady=(12,0))
        def callback_error(kind, value, trace):
            (args.run / 'playback-error.log').write_text(''.join(traceback.format_exception(kind, value, trace)))
            root.destroy()
        root.report_callback_exception = callback_error
        screen = tk.Label(root, bg='#080d13')
        screen.pack(padx=16, pady=(16, 8))
        status = tk.StringVar()
        tk.Label(root, textvariable=status, bg='#111c27', fg='#e0edf6', font=('Segoe UI', 11)).pack(padx=16, pady=8)
        controls = ttk.Frame(root)
        controls.pack(pady=(0, 16))
        agent_view = tk.Label(root, bg='#111c27')
        show_agent_view = tk.BooleanVar(value=args.agent_view)
        start_variant = args.start_variant
        observation = None
        paused = False
        finished = False
        total = 0
        episodes = 0
        restart_at = 0
        celebration_left = 0
        score_metric = next((m for m in env.task['metrics'] if m['name'] == 'score'), None)

        def draw():
            screen.photo = ImageTk.PhotoImage(Image.fromarray(env.render()).resize((720, 480), Image.Resampling.NEAREST))
            screen.configure(image=screen.photo)
            if hybrid and show_agent_view.get() and observation is not None:
                strip = np.concatenate(list(observation['image']), axis=1)
                agent_view.photo = ImageTk.PhotoImage(Image.fromarray(strip))
                agent_view.configure(image=agent_view.photo)
                agent_view.pack(padx=16, pady=(0,12))
            else:
                agent_view.pack_forget()

        def reset():
            nonlocal observation, total, finished, celebration_left
            observation, _ = env.reset(seed=manifest['seed'] + 10000 + episodes,
                                       options={'variant':start_variant} if start_variant is not None else None)
            total, finished = 0, False
            celebration_left = 0
            status.set(f'Deterministic policy · {args.run.name}')
            draw()

        def toggle():
            nonlocal paused
            paused = not paused
            pause_button.configure(text='Resume' if paused else 'Pause')

        pause_button = ttk.Button(controls, text='Pause', command=toggle)
        pause_button.pack(side='left', padx=4)
        ttk.Button(controls, text='Restart', command=reset).pack(side='left', padx=4)
        ttk.Button(controls, text='Close', command=root.destroy).pack(side='left', padx=4)
        if hybrid:
            ttk.Checkbutton(controls, text='Agent view', variable=show_agent_view, command=draw).pack(side='left', padx=4)
        variants = env.task.get('reset_variants', [])
        if len(variants) > 1:
            starts = ttk.Combobox(controls, state='readonly', width=15,
                                 values=['Random start'] + [f'Checkpoint {i+1}' for i in range(len(variants))])
            starts.current(0 if start_variant is None else start_variant + 1)
            starts.pack(side='left', padx=4)
            def choose_start(event):
                nonlocal start_variant
                start_variant = starts.current() - 1 if starts.current() else None
                reset()
            starts.bind('<<ComboboxSelected>>', choose_start)

        def tick():
            nonlocal observation, total, finished, episodes, restart_at, celebration_left
            started = time.monotonic()
            if not paused:
                if finished:
                    if celebration_left:
                        frames = min(4, celebration_left)
                        tally = env.client.step([], frames=frames)
                        celebration_left -= frames
                        env.last_action_frames = frames
                        env.cached_frame = None
                        draw()
                        score_text = f" | score {tally['memory'][score_metric['field']] * score_metric.get('scale', 1):,.0f}" if score_metric else ''
                        status.set(f"Success{score_text} | play time {env.elapsed_frames / 60:.1f}s | automatic finish animation")
                        restart_at = time.monotonic() + 2
                    elif time.monotonic() >= restart_at:
                        if args.episodes and episodes >= args.episodes:
                            root.destroy()
                            return
                        reset()
                else:
                    action = int(model.predict(observation, deterministic=True)[0])
                    observation, reward, terminated, truncated, info = env.step(action)
                    total += reward
                    draw()
                    buttons = ' + '.join(env.task['actions'][action]) or 'Wait'
                    position = info['memory'].get('player.world_x_fixed')
                    position_text = f' · x {position / 256:.1f}' if position is not None else ''
                    score = info.get('metrics', {}).get('score')
                    score_text = f' · score {score:,.0f}' if score is not None else ''
                    score_text += f" | {info.get('elapsed_seconds', 0):.1f}s"
                    if 'coins' in info.get('metrics', {}):
                        score_text += f" | coins {info['metrics']['coins']:.0f}"
                    status.set(f'{buttons}{position_text}{score_text} · reward {total:.2f} · episode {episodes + 1}')
                    if terminated or truncated:
                        finished = True
                        celebration_left = args.finish_animation_frames if info.get('is_success') else 0
                        episodes += 1
                        restart_at = time.monotonic() + 2
                        status.set(f"{info.get('outcome') or info.get('truncation_reason') or 'Time limit'}{score_text} · reward {total:.2f}{position_text} · restarting…")
            interval = max(1, env.last_action_frames) * 1000 / 60
            root.after(max(1, round(interval - (time.monotonic() - started) * 1000)), tick)

        reset()
        root.after(0, tick)
        root.mainloop()
    finally:
        env.close()
        if root is not None:
            try:
                root.destroy()
            except tk.TclError:
                pass


if __name__ == '__main__':
    main()
