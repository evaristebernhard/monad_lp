import yfinance as yf
import pandas as pd
import numpy as np
from typing import List, Dict, Optional, Tuple
import os
import logging
from datetime import datetime, timedelta
import pickle
from concurrent.futures import ThreadPoolExecutor, as_completed
import time
from sklearn.preprocessing import MinMaxScaler, StandardScaler
import warnings
warnings.filterwarnings('ignore')

# Set up logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

class StockDataProcessor:
    """
    A comprehensive class for downloading, processing, and preprocessing stock data
    from Yahoo Finance for reinforcement learning applications.
    """
    
    def __init__(self, data_dir: str = "stock_data", cache_dir: str = "cache"):
        self.data_dir = data_dir
        self.cache_dir = cache_dir
        self.scalers = {}
        
        # Create directories if they don't exist
        os.makedirs(data_dir, exist_ok=True)
        os.makedirs(cache_dir, exist_ok=True)
        
    def get_sp500_tickers(self) -> List[str]:
        """Get S&P 500 stock tickers"""
        try:
            # Download S&P 500 list from Wikipedia
            url = 'https://en.wikipedia.org/wiki/List_of_S%26P_500_companies'
            tables = pd.read_html(url)
            sp500_table = tables[0]
            tickers = sp500_table['Symbol'].tolist()
            # Clean tickers (remove dots, etc.)
            tickers = [ticker.replace('.', '-') for ticker in tickers]
            logger.info(f"Retrieved {len(tickers)} S&P 500 tickers")
            return tickers
        except Exception as e:
            logger.error(f"Error fetching S&P 500 tickers: {e}")
            # Fallback to a smaller list of popular stocks
            return ['AAPL', 'MSFT', 'GOOGL', 'AMZN', 'TSLA', 'META', 'NVDA', 'JPM', 'JNJ', 'V']
    
    def download_stock_data(self, 
                          ticker: str, 
                          period: str = "10y", 
                          interval: str = "1d") -> Optional[pd.DataFrame]:
        """
        Download stock data for a single ticker
        
        Args:
            ticker: Stock symbol
            period: Time period (1d, 5d, 1mo, 3mo, 6mo, 1y, 2y, 5y, 10y, ytd, max)
            interval: Data interval (1m, 2m, 5m, 15m, 30m, 60m, 90m, 1h, 1d, 5d, 1wk, 1mo, 3mo)
        """
        try:
            stock = yf.Ticker(ticker)
            data = stock.history(period=period, interval=interval)
            
            if data.empty:
                logger.warning(f"No data found for {ticker}")
                return None
                
            # Add ticker column
            data['Ticker'] = ticker
            data.reset_index(inplace=True)
            
            logger.info(f"Downloaded {len(data)} records for {ticker}")
            return data
            
        except Exception as e:
            logger.error(f"Error downloading data for {ticker}: {e}")
            return None
    
    def download_multiple_stocks(self, 
                                tickers: List[str], 
                                period: str = "10y",
                                interval: str = "1d",
                                max_workers: int = 10) -> pd.DataFrame:
        """
        Download stock data for multiple tickers using parallel processing
        """
        all_data = []
        
        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            # Submit all download tasks
            future_to_ticker = {
                executor.submit(self.download_stock_data, ticker, period, interval): ticker 
                for ticker in tickers
            }
            
            # Collect results
            for future in as_completed(future_to_ticker):
                ticker = future_to_ticker[future]
                try:
                    data = future.result()
                    if data is not None:
                        all_data.append(data)
                except Exception as e:
                    logger.error(f"Error processing {ticker}: {e}")
                
                # Rate limiting
                time.sleep(0.1)
        
        if all_data:
            combined_data = pd.concat(all_data, ignore_index=True)
            logger.info(f"Combined data shape: {combined_data.shape}")
            return combined_data
        else:
            return pd.DataFrame()
    
    def calculate_technical_indicators(self, df: pd.DataFrame) -> pd.DataFrame:
        """
        Calculate technical indicators for each stock
        """
        logger.info("Calculating technical indicators...")
        
        result_dfs = []
        
        for ticker in df['Ticker'].unique():
            ticker_data = df[df['Ticker'] == ticker].copy()
            ticker_data = ticker_data.sort_values('Date')
            
            # Moving averages
            ticker_data['SMA_5'] = ticker_data['Close'].rolling(window=5).mean()
            ticker_data['SMA_10'] = ticker_data['Close'].rolling(window=10).mean()
            ticker_data['SMA_20'] = ticker_data['Close'].rolling(window=20).mean()
            ticker_data['SMA_50'] = ticker_data['Close'].rolling(window=50).mean()
            
            # Exponential moving averages
            ticker_data['EMA_12'] = ticker_data['Close'].ewm(span=12).mean()
            ticker_data['EMA_26'] = ticker_data['Close'].ewm(span=26).mean()
            
            # MACD
            ticker_data['MACD'] = ticker_data['EMA_12'] - ticker_data['EMA_26']
            ticker_data['MACD_Signal'] = ticker_data['MACD'].ewm(span=9).mean()
            ticker_data['MACD_Histogram'] = ticker_data['MACD'] - ticker_data['MACD_Signal']
            
            # RSI
            delta = ticker_data['Close'].diff()
            gain = (delta.where(delta > 0, 0)).rolling(window=14).mean()
            loss = (-delta.where(delta < 0, 0)).rolling(window=14).mean()
            rs = gain / loss
            ticker_data['RSI'] = 100 - (100 / (1 + rs))
            
            # Bollinger Bands
            ticker_data['BB_Middle'] = ticker_data['Close'].rolling(window=20).mean()
            bb_std = ticker_data['Close'].rolling(window=20).std()
            ticker_data['BB_Upper'] = ticker_data['BB_Middle'] + (bb_std * 2)
            ticker_data['BB_Lower'] = ticker_data['BB_Middle'] - (bb_std * 2)
            ticker_data['BB_Width'] = ticker_data['BB_Upper'] - ticker_data['BB_Lower']
            ticker_data['BB_Position'] = (ticker_data['Close'] - ticker_data['BB_Lower']) / ticker_data['BB_Width']
            
            # Volatility
            ticker_data['Volatility'] = ticker_data['Close'].rolling(window=20).std()
            
            # Price change features
            ticker_data['Price_Change'] = ticker_data['Close'].pct_change()
            ticker_data['Price_Change_5d'] = ticker_data['Close'].pct_change(periods=5)
            ticker_data['High_Low_Ratio'] = ticker_data['High'] / ticker_data['Low']
            ticker_data['Open_Close_Ratio'] = ticker_data['Open'] / ticker_data['Close']
            
            # Volume features
            ticker_data['Volume_SMA'] = ticker_data['Volume'].rolling(window=20).mean()
            ticker_data['Volume_Ratio'] = ticker_data['Volume'] / ticker_data['Volume_SMA']
            
            result_dfs.append(ticker_data)
        
        result = pd.concat(result_dfs, ignore_index=True)
        logger.info(f"Technical indicators calculated. New shape: {result.shape}")
        return result
    
    def create_lagged_features(self, df: pd.DataFrame, lags: List[int] = [1, 2, 3, 5, 10]) -> pd.DataFrame:
        """
        Create lagged features for time series analysis
        """
        logger.info("Creating lagged features...")
        
        result_dfs = []
        feature_columns = ['Close', 'Volume', 'Price_Change', 'RSI', 'MACD', 'Volatility']
        
        for ticker in df['Ticker'].unique():
            ticker_data = df[df['Ticker'] == ticker].copy()
            ticker_data = ticker_data.sort_values('Date')
            
            for col in feature_columns:
                if col in ticker_data.columns:
                    for lag in lags:
                        ticker_data[f'{col}_lag_{lag}'] = ticker_data[col].shift(lag)
            
            result_dfs.append(ticker_data)
        
        result = pd.concat(result_dfs, ignore_index=True)
        logger.info(f"Lagged features created. New shape: {result.shape}")
        return result
    
    def create_future_returns(self, df: pd.DataFrame, horizons: List[int] = [1, 5, 10, 20]) -> pd.DataFrame:
        """
        Create future return targets for prediction
        """
        logger.info("Creating future return targets...")
        
        result_dfs = []
        
        for ticker in df['Ticker'].unique():
            ticker_data = df[df['Ticker'] == ticker].copy()
            ticker_data = ticker_data.sort_values('Date')
            
            for horizon in horizons:
                ticker_data[f'Future_Return_{horizon}d'] = ticker_data['Close'].shift(-horizon) / ticker_data['Close'] - 1
                
                # Create binary classification targets
                ticker_data[f'Future_Up_{horizon}d'] = (ticker_data[f'Future_Return_{horizon}d'] > 0).astype(int)
                
                # Create categorical targets (strong down, down, up, strong up)
                returns = ticker_data[f'Future_Return_{horizon}d']
                ticker_data[f'Future_Category_{horizon}d'] = pd.cut(
                    returns, 
                    bins=[-np.inf, -0.02, 0, 0.02, np.inf], 
                    labels=[0, 1, 2, 3]
                ).astype(float)
            
            result_dfs.append(ticker_data)
        
        result = pd.concat(result_dfs, ignore_index=True)
        logger.info(f"Future return targets created. New shape: {result.shape}")
        return result
    
    def clean_and_normalize_data(self, df: pd.DataFrame) -> pd.DataFrame:
        """
        Clean and normalize the data for ML/RL
        """
        logger.info("Cleaning and normalizing data...")
        
        # Remove rows with too many NaN values
        df = df.dropna(thresh=len(df.columns) * 0.7)
        
        # Forward fill remaining NaN values
        numeric_columns = df.select_dtypes(include=[np.number]).columns
        df[numeric_columns] = df[numeric_columns].fillna(method='ffill')
        
        # Remove infinite values
        df = df.replace([np.inf, -np.inf], np.nan)
        df = df.dropna()
        
        logger.info(f"Data cleaned. Final shape: {df.shape}")
        return df
    
    def create_rl_states_actions(self, df: pd.DataFrame) -> Dict:
        """
        Create state and action spaces suitable for reinforcement learning
        """
        logger.info("Creating RL state and action representations...")
        
        # Define state features (technical indicators and market data)
        state_features = [
            'Open', 'High', 'Low', 'Close', 'Volume',
            'SMA_5', 'SMA_10', 'SMA_20', 'SMA_50',
            'EMA_12', 'EMA_26', 'MACD', 'MACD_Signal', 'RSI',
            'BB_Position', 'BB_Width', 'Volatility',
            'Price_Change', 'High_Low_Ratio', 'Volume_Ratio'
        ]
        
        # Add lagged features to state
        lag_features = [col for col in df.columns if '_lag_' in col]
        state_features.extend(lag_features)
        
        # Filter existing features
        state_features = [feat for feat in state_features if feat in df.columns]
        
        # Normalize state features
        scaler = StandardScaler()
        df_scaled = df.copy()
        df_scaled[state_features] = scaler.fit_transform(df[state_features])
        
        # Define action space (0: Hold, 1: Buy, 2: Sell)
        # You can expand this based on your RL strategy
        
        # Create sequences for each stock
        rl_data = {}
        sequence_length = 60  # Number of days to look back
        
        for ticker in df_scaled['Ticker'].unique():
            ticker_data = df_scaled[df_scaled['Ticker'] == ticker].sort_values('Date')
            
            states = []
            rewards = []
            dates = []
            
            for i in range(sequence_length, len(ticker_data)):
                # State: sequence of technical indicators
                state_sequence = ticker_data.iloc[i-sequence_length:i][state_features].values
                states.append(state_sequence)
                
                # Reward: next day return (can be modified based on your RL objective)
                if 'Future_Return_1d' in ticker_data.columns:
                    reward = ticker_data.iloc[i]['Future_Return_1d']
                else:
                    current_price = ticker_data.iloc[i]['Close']
                    if i < len(ticker_data) - 1:
                        next_price = ticker_data.iloc[i+1]['Close']
                        reward = (next_price - current_price) / current_price
                    else:
                        reward = 0
                
                rewards.append(reward)
                dates.append(ticker_data.iloc[i]['Date'])
            
            rl_data[ticker] = {
                'states': np.array(states),
                'rewards': np.array(rewards),
                'dates': dates,
                'state_features': state_features
            }
        
        logger.info(f"RL data created for {len(rl_data)} stocks")
        return rl_data, scaler
    
    def save_processed_data(self, data: pd.DataFrame, rl_data: Dict, scaler, filename_prefix: str = "processed_stock_data"):
        """
        Save processed data to files
        """
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        
        # Save CSV data
        csv_filename = f"{self.data_dir}/{filename_prefix}_{timestamp}.csv"
        data.to_csv(csv_filename, index=False)
        logger.info(f"CSV data saved to {csv_filename}")
        
        # Save RL data
        rl_filename = f"{self.data_dir}/{filename_prefix}_rl_{timestamp}.pkl"
        with open(rl_filename, 'wb') as f:
            pickle.dump(rl_data, f)
        logger.info(f"RL data saved to {rl_filename}")
        
        # Save scaler
        scaler_filename = f"{self.data_dir}/{filename_prefix}_scaler_{timestamp}.pkl"
        with open(scaler_filename, 'wb') as f:
            pickle.dump(scaler, f)
        logger.info(f"Scaler saved to {scaler_filename}")
        
        return csv_filename, rl_filename, scaler_filename
    
    def process_stocks_pipeline(self, 
                               tickers: Optional[List[str]] = None,
                               period: str = "10y",
                               interval: str = "1d",
                               use_sp500: bool = True) -> Tuple[pd.DataFrame, Dict, object]:
        """
        Complete pipeline for processing stock data
        """
        logger.info("Starting stock data processing pipeline...")
        
        # Get tickers
        if tickers is None:
            if use_sp500:
                tickers = self.get_sp500_tickers()
            else:
                tickers = ['AAPL', 'MSFT', 'GOOGL', 'AMZN', 'TSLA']  # Default list
        
        # Download data
        logger.info(f"Downloading data for {len(tickers)} tickers...")
        raw_data = self.download_multiple_stocks(tickers, period, interval)
        
        if raw_data.empty:
            logger.error("No data downloaded. Exiting.")
            return None, None, None
        
        # Process data
        data_with_indicators = self.calculate_technical_indicators(raw_data)
        data_with_lags = self.create_lagged_features(data_with_indicators)
        data_with_targets = self.create_future_returns(data_with_lags)
        cleaned_data = self.clean_and_normalize_data(data_with_targets)
        
        # Create RL data
        rl_data, scaler = self.create_rl_states_actions(cleaned_data)
        
        # Save data
        self.save_processed_data(cleaned_data, rl_data, scaler)
        
        logger.info("Pipeline completed successfully!")
        return cleaned_data, rl_data, scaler

# Example usage and utility functions
def example_usage():
    """
    Example of how to use the StockDataProcessor
    """
    # Initialize processor
    processor = StockDataProcessor()
    
    # Option 1: Process S&P 500 stocks
    print("Processing S&P 500 stocks...")
    data, rl_data, scaler = processor.process_stocks_pipeline(use_sp500=True, period="5y")
    
    # Option 2: Process specific stocks
    # custom_tickers = ['AAPL', 'MSFT', 'GOOGL', 'AMZN', 'TSLA', 'META', 'NVDA']
    # data, rl_data, scaler = processor.process_stocks_pipeline(tickers=custom_tickers, period="10y")
    
    if data is not None:
        print(f"Processed data shape: {data.shape}")
        print(f"Features: {data.columns.tolist()}")
        print(f"RL data available for {len(rl_data)} stocks")
        
        # Example: Access RL data for a specific stock
        if 'AAPL' in rl_data:
            aapl_states = rl_data['AAPL']['states']
            aapl_rewards = rl_data['AAPL']['rewards']
            print(f"AAPL: {aapl_states.shape[0]} sequences, each with {aapl_states.shape[1]} timesteps and {aapl_states.shape[2]} features")

def load_processed_data(rl_filename: str, scaler_filename: str) -> Tuple[Dict, object]:
    """
    Load previously processed RL data
    """
    with open(rl_filename, 'rb') as f:
        rl_data = pickle.load(f)
    
    with open(scaler_filename, 'rb') as f:
        scaler = pickle.load(f)
    
    return rl_data, scaler

if __name__ == "__main__":
    example_usage()