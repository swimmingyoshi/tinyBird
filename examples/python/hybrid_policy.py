"""A spatial CNN and RAM encoder for local visual reinforcement learning."""
import torch
from torch import nn
from stable_baselines3.common.torch_layers import BaseFeaturesExtractor


class HybridExtractor(BaseFeaturesExtractor):
    def __init__(self, observation_space):
        super().__init__(observation_space, features_dim=192)
        channels, height, width = observation_space['image'].shape
        self.vision = nn.Sequential(
            nn.Conv2d(channels, 16, 5, stride=2), nn.ReLU(),
            nn.Conv2d(16, 32, 3, stride=2), nn.ReLU(),
            nn.Conv2d(32, 32, 3, stride=2), nn.ReLU(), nn.Flatten())
        with torch.no_grad():
            size = self.vision(torch.zeros(1, channels, height, width)).shape[1]
        self.visual_projection = nn.Sequential(nn.Linear(size, 128), nn.ReLU())
        self.ram = nn.Sequential(nn.Linear(observation_space['ram'].shape[0], 64), nn.ReLU())

    def forward(self, observations):
        # SB3 normalizes uint8 image observations to [0, 1] before this call.
        return torch.cat((self.visual_projection(self.vision(observations['image'])),
                          self.ram(observations['ram'])), dim=1)
