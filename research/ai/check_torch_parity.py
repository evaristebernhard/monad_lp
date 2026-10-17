"""Compare checkpoint actor FP32 in PyTorch with exported reference numerics."""
import io
import json
import random
import zipfile
import torch
import study_checkpoint as study


def main():
    study.verify()
    with zipfile.ZipFile(study.MODEL / 'final_model.zip') as checkpoint:
        state = torch.load(io.BytesIO(checkpoint.read('policy.pth')), map_location='cpu', weights_only=True)
    net = torch.nn.Sequential(
        torch.nn.Linear(3008, 64), torch.nn.Tanh(), torch.nn.Linear(64, 64),
        torch.nn.Tanh(), torch.nn.Linear(64, 2),
    )
    for index, prefix in [(0, 'mlp_extractor.policy_net.0'), (2, 'mlp_extractor.policy_net.2'), (4, 'action_net')]:
        net[index].load_state_dict({'weight': state[prefix + '.weight'], 'bias': state[prefix + '.bias']})
    net.eval()
    _, weights = study.read_weights()
    layers = study.layers_from(weights)
    rng = random.Random(study.SEED)
    inputs = [[0.0] * 3008, [0.0] * 3000 + [1, 0, 1, 0, 0, 0, 0, 0], [1.0] * 3008, [-1.0] * 3008]
    for i in range(study.SAMPLES - len(inputs)):
        amp = [0.1, 1.0, 3.0, 8.0][i % 4]
        inputs.append([rng.uniform(-amp, amp) for _ in range(3008)])
    with torch.no_grad():
        outputs = net(torch.tensor(inputs, dtype=torch.float32)).tolist()
    reference = [study.forward_float(layers, obs) for obs in inputs]
    report = dict(
        torch_version=torch.__version__,
        method='weights_only load; ActorCriticPolicy deterministic actor Linear/Tanh/Linear/Tanh/Linear',
        input_kind='64 synthetic normalized stress vectors; not MON observations',
        samples=len(inputs),
        reference_vs_pytorch_max_raw_output_error=max(abs(x-y) for a,b in zip(reference,outputs) for x,y in zip(a,b)),
        reference_vs_pytorch_action_disagreements=sum(study.decode(a)[0] != study.decode(b)[0] for a,b in zip(reference,outputs)),
        quantization={},
    )
    table = json.loads((study.OUT / 'tanh-q16.json').read_text())
    for bits in [8, 16]:
        qlayers = study.quantize(layers, bits)
        result = [study.forward_int(qlayers, obs, table) for obs in inputs]
        report['quantization'][str(bits)] = dict(
            max_raw_output_error_vs_pytorch=max(abs(x-y) for a,b in zip(result,outputs) for x,y in zip(a,b)),
            action_disagreements_vs_pytorch=sum(study.decode(a)[0] != study.decode(b)[0] for a,b in zip(result,outputs)),
        )
    assert report['reference_vs_pytorch_max_raw_output_error'] < 0.0001
    (study.OUT / 'pytorch-parity.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
