import numpy as np
import pandas as pd
import gymnasium as gym
from gymnasium import spaces
from typing import Dict, Tuple, List, Optional
import logging
from dataclasses import dataclass
from enum import Enum
import matplotlib.pyplot as plt
import seaborn as sns
from collections import deque
import json

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

class ActionType(Enum):
    HOLD = 0
    BUY = 1
    SELL = 2

@dataclass
class TradingMetrics:
    """Comprehensive trading metrics for evaluation"""
    total_return: float = 0.0
    sharpe_ratio: float = 0.0
    max_drawdown: float = 0.0
    win_rate: float = 0.0
    total_trades: int = 0
    profitable_trades: int = 0
    average_trade_return: float = 0.0
    volatility: float = 0.0
    calmar_ratio: float = 0.0
    sortino_ratio: float = 0.0

class EnhancedStockTradingEnvironment(gym.Env):
    """
    Enhanced stock trading environment with comprehensive metrics and logging
    """
    
    def __init__(self, 
                 rl_data: Dict, 
                 ticker: str,
                 initial_balance: float = 10000,
                 transaction_cost: float = 0.001,  # 0.1% transaction cost
                 max_position_size: float = 1.0,   # Maximum position size as fraction of portfolio
                 lookback_window: int = 60,        # Number of days to look back
                 reward_type: str = "return",      # "return", "sharpe", "sortino"
                 enable_logging: bool = True):
        
        super().__init__()
        
        self.rl_data = rl_data
        self.ticker = ticker
        self.initial_balance = initial_balance
        self.transaction_cost = transaction_cost
        self.max_position_size = max_position_size
        self.lookback_window = lookback_window
        self.reward_type = reward_type
        self.enable_logging = enable_logging
        
        # Get data for the specific ticker
        self.stock_data = rl_data[ticker]
        self.states = self.stock_data['states']
        self.prices = self._extract_prices()  # Extract actual prices
        self.dates = self.stock_data['dates']
        
        # Environment parameters
        self.current_step = 0
        self.max_steps = len(self.states) - 1
        
        # Portfolio state
        self.reset_portfolio()
        
        # Trading history
        self.trade_history = []
        self.portfolio_history = []
        self.action_history = []
        self.reward_history = []
        
        # Performance tracking
        self.daily_returns = deque(maxlen=252)  # 1 year of returns for Sharpe calculation
        self.drawdown_history = []
        self.peak_portfolio_value = initial_balance
        
        # Action space: 0 = Hold, 1 = Buy, 2 = Sell, with continuous position sizing
        self.action_space = spaces.Box(
            low=np.array([0, 0]),        # [action_type (0-2), position_size (0-1)]
            high=np.array([2, 1]),
            dtype=np.float32
        )
        
        # Observation space: market state + portfolio state + technical indicators
        market_state_size = self.states.shape[1] * self.states.shape[2]
        portfolio_state_size = 8  # Extended portfolio state
        
        self.observation_space = spaces.Box(
            low=-np.inf,
            high=np.inf,
            shape=(market_state_size + portfolio_state_size,),
            dtype=np.float32
        )
        
        if self.enable_logging:
            logger.info(f"Environment initialized for {ticker}")
            logger.info(f"Data shape: {self.states.shape}")
            logger.info(f"Price range: ${self.prices.min():.2f} - ${self.prices.max():.2f}")
    
    def _extract_prices(self) -> np.ndarray:
        """Extract actual prices from the state data"""
        # Assuming the first feature in states is the close price
        return self.states[:, -1, 3]  # Close price is typically at index 3
    
    def reset_portfolio(self):
        """Reset portfolio to initial state"""
        self.balance = self.initial_balance
        self.shares_held = 0
        self.net_worth = self.initial_balance
        self.max_net_worth = self.initial_balance
        self.position_value = 0
        self.total_transaction_costs = 0
        
    def reset(self, seed=None, options=None):
        super().reset(seed=seed)
        
        self.current_step = 0
        self.reset_portfolio()
        
        # Clear histories
        self.trade_history.clear()
        self.portfolio_history.clear()
        self.action_history.clear()
        self.reward_history.clear()
        self.daily_returns.clear()
        self.drawdown_history.clear()
        self.peak_portfolio_value = self.initial_balance
        
        return self._get_observation(), {}
    
    def step(self, action):
        # Parse action
        action_type = int(np.clip(action[0], 0, 2))
        position_size = np.clip(action[1], 0, 1)
        
        # Execute action
        reward = self._execute_action(action_type, position_size)
        
        # Update portfolio metrics
        self._update_portfolio_metrics()
        
        # Store history
        self._store_step_data(action_type, position_size, reward)
        
        # Move to next step
        self.current_step += 1
        
        # Check if episode is done
        done = self.current_step >= self.max_steps
        truncated = False
        
        # Calculate final metrics if done
        info = {}
        if done:
            info = self._calculate_episode_metrics()
        
        return self._get_observation(), reward, done, truncated, info
    
    def _execute_action(self, action_type: int, position_size: float) -> float:
        """Execute trading action and return reward"""
        current_price = self.prices[self.current_step]
        previous_net_worth = self.net_worth
        
        if action_type == ActionType.BUY.value:
            # Calculate how much to buy
            max_affordable = self.balance / current_price
            shares_to_buy = int(max_affordable * position_size)
            
            if shares_to_buy > 0:
                cost = shares_to_buy * current_price
                transaction_cost = cost * self.transaction_cost
                
                if self.balance >= cost + transaction_cost:
                    self.shares_held += shares_to_buy
                    self.balance -= (cost + transaction_cost)
                    self.total_transaction_costs += transaction_cost
                    
                    self.trade_history.append({
                        'step': self.current_step,
                        'action': 'BUY',
                        'shares': shares_to_buy,
                        'price': current_price,
                        'cost': cost,
                        'transaction_cost': transaction_cost
                    })
        
        elif action_type == ActionType.SELL.value:
            # Calculate how much to sell
            shares_to_sell = int(self.shares_held * position_size)
            
            if shares_to_sell > 0:
                revenue = shares_to_sell * current_price
                transaction_cost = revenue * self.transaction_cost
                
                self.shares_held -= shares_to_sell
                self.balance += (revenue - transaction_cost)
                self.total_transaction_costs += transaction_cost
                
                self.trade_history.append({
                    'step': self.current_step,
                    'action': 'SELL',
                    'shares': shares_to_sell,
                    'price': current_price,
                    'revenue': revenue,
                    'transaction_cost': transaction_cost
                })
        
        # Calculate new net worth
        self.position_value = self.shares_held * current_price
        self.net_worth = self.balance + self.position_value
        
        # Calculate reward based on selected method
        reward = self._calculate_reward(previous_net_worth)
        
        return reward
    
    def _calculate_reward(self, previous_net_worth: float) -> float:
        """Calculate reward based on the selected reward type"""
        if self.reward_type == "return":
            # Simple return-based reward
            return (self.net_worth - previous_net_worth) / previous_net_worth
        
        elif self.reward_type == "sharpe":
            # Sharpe ratio-based reward
            if len(self.daily_returns) > 1:
                returns = np.array(self.daily_returns)
                if np.std(returns) > 0:
                    sharpe = np.mean(returns) / np.std(returns) * np.sqrt(252)
                    return sharpe / 100  # Scale down
            return 0
        
        elif self.reward_type == "sortino":
            # Sortino ratio-based reward
            if len(self.daily_returns) > 1:
                returns = np.array(self.daily_returns)
                negative_returns = returns[returns < 0]
                if len(negative_returns) > 0 and np.std(negative_returns) > 0:
                    sortino = np.mean(returns) / np.std(negative_returns) * np.sqrt(252)
                    return sortino / 100  # Scale down
            return 0
        
        else:
            return (self.net_worth - previous_net_worth) / previous_net_worth
    
    def _update_portfolio_metrics(self):
        """Update portfolio performance metrics"""
        # Calculate daily return
        if len(self.portfolio_history) > 0:
            daily_return = (self.net_worth - self.portfolio_history[-1]['net_worth']) / self.portfolio_history[-1]['net_worth']
            self.daily_returns.append(daily_return)
        
        # Update peak and drawdown
        if self.net_worth > self.peak_portfolio_value:
            self.peak_portfolio_value = self.net_worth
        
        current_drawdown = (self.peak_portfolio_value - self.net_worth) / self.peak_portfolio_value
        self.drawdown_history.append(current_drawdown)
    
    def _store_step_data(self, action_type: int, position_size: float, reward: float):
        """Store data for analysis"""
        self.action_history.append({
            'step': self.current_step,
            'action_type': action_type,
            'position_size': position_size
        })
        
        self.portfolio_history.append({
            'step': self.current_step,
            'balance': self.balance,
            'shares_held': self.shares_held,
            'position_value': self.position_value,
            'net_worth': self.net_worth,
            'price': self.prices[self.current_step]
        })
        
        self.reward_history.append(reward)
    
    def _calculate_episode_metrics(self) -> Dict:
        """Calculate comprehensive episode metrics"""
        if len(self.portfolio_history) == 0:
            return {}
        
        # Basic returns
        total_return = (self.net_worth - self.initial_balance) / self.initial_balance
        
        # Risk metrics
        returns = np.array(self.daily_returns) if self.daily_returns else np.array([0])
        max_drawdown = max(self.drawdown_history) if self.drawdown_history else 0
        volatility = np.std(returns) * np.sqrt(252)
        
        # Sharpe ratio
        sharpe_ratio = np.mean(returns) / np.std(returns) * np.sqrt(252) if np.std(returns) > 0 else 0
        
        # Sortino ratio
        negative_returns = returns[returns < 0]
        sortino_ratio = np.mean(returns) / np.std(negative_returns) * np.sqrt(252) if len(negative_returns) > 0 and np.std(negative_returns) > 0 else 0
        
        # Calmar ratio
        calmar_ratio = (np.mean(returns) * 252) / max_drawdown if max_drawdown > 0 else 0
        
        # Trading metrics
        total_trades = len(self.trade_history)
        buy_trades = [t for t in self.trade_history if t['action'] == 'BUY']
        sell_trades = [t for t in self.trade_history if t['action'] == 'SELL']
        
        # Win rate calculation (simplified)
        profitable_trades = len([r for r in self.reward_history if r > 0])
        win_rate = profitable_trades / len(self.reward_history) if len(self.reward_history) > 0 else 0
        
        metrics = {
            'total_return': total_return,
            'sharpe_ratio': sharpe_ratio,
            'sortino_ratio': sortino_ratio,
            'calmar_ratio': calmar_ratio,
            'max_drawdown': max_drawdown,
            'volatility': volatility,
            'win_rate': win_rate,
            'total_trades': total_trades,
            'buy_trades': len(buy_trades),
            'sell_trades': len(sell_trades),
            'final_balance': self.balance,
            'final_shares': self.shares_held,
            'final_net_worth': self.net_worth,
            'total_transaction_costs': self.total_transaction_costs,
            'average_reward': np.mean(self.reward_history) if self.reward_history else 0
        }
        
        if self.enable_logging:
            logger.info(f"Episode completed for {self.ticker}")
            logger.info(f"Total Return: {total_return:.2%}")
            logger.info(f"Sharpe Ratio: {sharpe_ratio:.2f}")
            logger.info(f"Max Drawdown: {max_drawdown:.2%}")
            logger.info(f"Win Rate: {win_rate:.2%}")
        
        return metrics
    
    def _get_observation(self):
        """Get current observation"""
        if self.current_step >= len(self.states):
            # Return last available state if we're at the end
            market_state = self.states[-1].flatten()
        else:
            market_state = self.states[self.current_step].flatten()
        
        # Portfolio state (normalized)
        current_price = self.prices[min(self.current_step, len(self.prices)-1)]
        
        portfolio_state = np.array([
            self.balance / self.initial_balance,                    # Normalized balance
            self.shares_held * current_price / self.initial_balance, # Normalized position value
            self.net_worth / self.initial_balance,                  # Normalized net worth
            (self.net_worth - self.initial_balance) / self.initial_balance, # Return
            len(self.trade_history) / 100,                          # Number of trades (normalized)
            self.total_transaction_costs / self.initial_balance,    # Transaction costs
            max(self.drawdown_history) if self.drawdown_history else 0, # Current max drawdown
            np.std(self.daily_returns) if len(self.daily_returns) > 1 else 0 # Volatility
        ])
        
        return np.concatenate([market_state, portfolio_state]).astype(np.float32)
    
    def render(self, mode='human'):
        """Render environment state"""
        current_price = self.prices[min(self.current_step, len(self.prices)-1)]
        
        print(f"\n=== {self.ticker} Trading Environment ===")
        print(f"Step: {self.current_step}/{self.max_steps}")
        print(f"Current Price: ${current_price:.2f}")
        print(f"Balance: ${self.balance:.2f}")
        print(f"Shares Held: {self.shares_held}")
        print(f"Position Value: ${self.position_value:.2f}")
        print(f"Net Worth: ${self.net_worth:.2f}")
        print(f"Total Return: {((self.net_worth - self.initial_balance) / self.initial_balance):.2%}")
        print(f"Total Trades: {len(self.trade_history)}")
        print(f"Transaction Costs: ${self.total_transaction_costs:.2f}")
        
        if self.drawdown_history:
            print(f"Max Drawdown: {max(self.drawdown_history):.2%}")
        
        print("=" * 40)
    
    def plot_performance(self, save_path: Optional[str] = None):
        """Plot comprehensive performance metrics"""
        if len(self.portfolio_history) == 0:
            print("No data to plot")
            return
        
        fig, axes = plt.subplots(2, 2, figsize=(15, 10))
        fig.suptitle(f'{self.ticker} Trading Performance', fontsize=16)
        
        # Portfolio value over time
        steps = [p['step'] for p in self.portfolio_history]
        net_worths = [p['net_worth'] for p in self.portfolio_history]
        prices = [p['price'] for p in self.portfolio_history]
        
        axes[0, 0].plot(steps, net_worths, label='Portfolio Value', linewidth=2)
        axes[0, 0].axhline(y=self.initial_balance, color='r', linestyle='--', label='Initial Balance')
        axes[0, 0].set_title('Portfolio Value Over Time')
        axes[0, 0].set_xlabel('Time Steps')
        axes[0, 0].set_ylabel('Portfolio Value ($)')
        axes[0, 0].legend()
        axes[0, 0].grid(True)
        
        # Stock price over time
        axes[0, 1].plot(steps, prices, label='Stock Price', color='orange', linewidth=2)
        axes[0, 1].set_title('Stock Price Over Time')
        axes[0, 1].set_xlabel('Time Steps')
        axes[0, 1].set_ylabel('Price ($)')
        axes[0, 1].legend()
        axes[0, 1].grid(True)
        
        # Drawdown
        if self.drawdown_history:
            axes[1, 0].fill_between(range(len(self.drawdown_history)), 
                                   self.drawdown_history, 0, 
                                   alpha=0.3, color='red')
            axes[1, 0].plot(self.drawdown_history, color='red', linewidth=2)
            axes[1, 0].set_title('Drawdown Over Time')
            axes[1, 0].set_xlabel('Time Steps')
            axes[1, 0].set_ylabel('Drawdown')
            axes[1, 0].grid(True)
        
        # Action distribution
        actions = [a['action_type'] for a in self.action_history]
        action_counts = [actions.count(i) for i in range(3)]
        action_labels = ['Hold', 'Buy', 'Sell']
        
        axes[1, 1].pie(action_counts, labels=action_labels, autopct='%1.1f%%')
        axes[1, 1].set_title('Action Distribution')
        
        plt.tight_layout()
        
        if save_path:
            plt.savefig(save_path, dpi=300, bbox_inches='tight')
            logger.info(f"Performance plot saved to {save_path}")
        
        plt.show()
    
    def get_metrics_summary(self) -> TradingMetrics:
        """Get trading metrics as a structured object"""
        metrics_dict = self._calculate_episode_metrics()
        
        return TradingMetrics(
            total_return=metrics_dict.get('total_return', 0),
            sharpe_ratio=metrics_dict.get('sharpe_ratio', 0),
            max_drawdown=metrics_dict.get('max_drawdown', 0),
            win_rate=metrics_dict.get('win_rate', 0),
            total_trades=metrics_dict.get('total_trades', 0),
            profitable_trades=int(metrics_dict.get('win_rate', 0) * metrics_dict.get('total_trades', 0)),
            average_trade_return=metrics_dict.get('average_reward', 0),
            volatility=metrics_dict.get('volatility', 0),
            calmar_ratio=metrics_dict.get('calmar_ratio', 0),
            sortino_ratio=metrics_dict.get('sortino_ratio', 0)
        )