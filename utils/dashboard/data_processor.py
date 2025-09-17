"""
Data processing functionality for the Rusty-Comms Dashboard

Handles discovery, loading, and processing of benchmark result files,
plus helper functions for directory browsing.
"""

import json
import logging
import pandas as pd
import numpy as np
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Dict, List
import os

logger = logging.getLogger(__name__)

class BenchmarkDataProcessor:
    """Handles discovery, loading, and processing of benchmark result files."""
    
    def __init__(self, results_dir: Path):
        self.results_dir = Path(results_dir)
        self.data_store = {
            'summary_data': pd.DataFrame(),
            'streaming_data': pd.DataFrame(),
            'runs_metadata': {}
        }
    
    def discover_files(self) -> Dict[str, List[Path]]:
        """Discover and categorize all benchmark result files."""
        files = {
            'summary_json': [],
            'streaming_json': [],
            'streaming_csv': [],
            'unknown': []
        }
        
        for file_path in self.results_dir.rglob('*.json'):
            try:
                file_type = self._detect_json_type(file_path)
                if file_type in ['summary_json', 'streaming_json']:
                    files[file_type].append(file_path)
                else:
                    files['unknown'].append(file_path)
            except Exception as e:
                logger.warning(f"Could not process {file_path}: {e}")
                files['unknown'].append(file_path)
        
        for file_path in self.results_dir.rglob('*.csv'):
            try:
                if self._detect_csv_type(file_path):
                    files['streaming_csv'].append(file_path)
                else:
                    files['unknown'].append(file_path)
            except Exception as e:
                logger.warning(f"Could not process {file_path}: {e}")
                files['unknown'].append(file_path)
        
        return files
    
    def _detect_json_type(self, file_path: Path) -> str:
        """Detect JSON file type based on structure."""
        try:
            with open(file_path, 'r') as f:
                data = json.load(f)
            
            # Summary JSON: has "results" with elements containing "summary"
            if isinstance(data, dict) and "results" in data:
                if isinstance(data["results"], list) and len(data["results"]) > 0:
                    if "summary" in data["results"][0]:
                        return "summary_json"
            
            # Streaming JSON: has "headings" and "data" keys
            if isinstance(data, dict) and "headings" in data and "data" in data:
                return "streaming_json"
                
            return "unknown"
        except (json.JSONDecodeError, KeyError):
            return "unknown"
    
    def _detect_csv_type(self, file_path: Path) -> bool:
        """Detect if CSV is streaming data based on columns."""
        try:
            df_sample = pd.read_csv(file_path, nrows=1)
            streaming_columns = {'one_way_latency_ns', 'round_trip_latency_ns', 'timestamp_ns', 'message_id'}
            return bool(streaming_columns.intersection(set(df_sample.columns)))
        except Exception:
            return False
    
    def _process_single_summary_file(self, file_path: Path) -> List[Dict]:
        """Process a single summary JSON file and return list of records."""
        try:
            with open(file_path, 'r') as f:
                data = json.load(f)
            
            records = []
            for result in data.get('results', []):
                mechanism = result.get('mechanism', 'unknown')
                # message_size is inside test_config
                test_config = result.get('test_config', {})
                message_size = test_config.get('message_size')
                if message_size is None or message_size <= 0:
                    logger.warning(f"Skipping result with invalid message_size: {message_size}")
                    continue
                
                summary = result.get('summary', {})
                
                # Extract latency percentiles (convert ns to μs)
                latency_data = summary.get('latency', {}).get('percentiles', [])
                percentiles = {p['percentile']: p['value_ns'] / 1000 for p in latency_data}
                
                # Extract throughput data
                one_way_throughput = result.get('one_way_results', {}).get('throughput', {})
                round_trip_throughput = result.get('round_trip_results', {}).get('throughput', {})
                
                record = {
                    'mechanism': mechanism,
                    'message_size': message_size,
                    'file_path': str(file_path),
                    'p50_latency_us': percentiles.get(50.0, np.nan),
                    'p95_latency_us': percentiles.get(95.0, np.nan),
                    'p99_latency_us': percentiles.get(99.0, np.nan),
                    'mean_latency_us': summary.get('latency', {}).get('mean_ns', 0) / 1000,
                    'one_way_msgs_per_sec': one_way_throughput.get('messages_per_second', np.nan),
                    'one_way_bytes_per_sec': one_way_throughput.get('bytes_per_second', np.nan),
                    'round_trip_msgs_per_sec': round_trip_throughput.get('messages_per_second', np.nan),
                    'round_trip_bytes_per_sec': round_trip_throughput.get('bytes_per_second', np.nan),
                }
                records.append(record)
                
            return records
                
        except Exception as e:
            logger.error(f"Error processing summary file {file_path}: {e}")
            return []

    def _process_single_streaming_json_file(self, file_path: Path) -> pd.DataFrame:
        """Process a single streaming JSON file and return DataFrame."""
        try:
            with open(file_path, 'r') as f:
                data = json.load(f)
            
            headings = data.get('headings', [])
            records = data.get('data', [])
            
            if not headings or not records:
                return pd.DataFrame()
            
            df = pd.DataFrame(records, columns=headings)
            
            # Detect mechanism and message size from first record
            if len(df) > 0:
                mechanism = df['mechanism'].iloc[0] if 'mechanism' in df.columns else 'unknown'
                message_size = df['message_size'].iloc[0] if 'message_size' in df.columns else None
                
                if message_size is None or message_size <= 0:
                    logger.warning(f"Skipping streaming file {file_path} with invalid message_size: {message_size}")
                    return pd.DataFrame()
                
                # Convert nanoseconds to microseconds for latency columns
                if 'one_way_latency_ns' in df.columns:
                    df['one_way_latency_us'] = df['one_way_latency_ns'] / 1000
                if 'round_trip_latency_ns' in df.columns:
                    df['round_trip_latency_us'] = df['round_trip_latency_ns'] / 1000
                    
                # Add file source for tracking
                df['source_file'] = str(file_path)
                
            return df
            
        except Exception as e:
            logger.error(f"Error processing streaming JSON file {file_path}: {e}")
            return pd.DataFrame()

    def _process_single_streaming_csv_file(self, file_path: Path) -> pd.DataFrame:
        """Process a single streaming CSV file and return DataFrame."""
        try:
            df = pd.read_csv(file_path)
            
            # Detect mechanism and message size from first row
            if len(df) > 0:
                mechanism = df['mechanism'].iloc[0] if 'mechanism' in df.columns else 'unknown'
                message_size = df['message_size'].iloc[0] if 'message_size' in df.columns else None
                
                if message_size is None or message_size <= 0:
                    logger.warning(f"Skipping streaming CSV file {file_path} with invalid message_size: {message_size}")
                    return pd.DataFrame()
                
                # Convert nanoseconds to microseconds for latency columns
                if 'one_way_latency_ns' in df.columns:
                    df['one_way_latency_us'] = df['one_way_latency_ns'] / 1000
                if 'round_trip_latency_ns' in df.columns:
                    df['round_trip_latency_us'] = df['round_trip_latency_ns'] / 1000
                    
                # Add file source for tracking
                df['source_file'] = str(file_path)
                
            return df
            
        except Exception as e:
            logger.error(f"Error processing streaming CSV file {file_path}: {e}")
            return pd.DataFrame()

    
    def load_summary_data(self, files: List[Path]) -> pd.DataFrame:
        """Load and process summary JSON files using threading for improved performance."""
        if not files:
            return pd.DataFrame()
        
        logger.info(f"Loading {len(files)} summary files using threading...")
        all_records = []
        
        # Use ThreadPoolExecutor to process files concurrently
        # Limit max workers to avoid overwhelming system resources
        max_workers = min(len(files), 8)  # Cap at 8 threads
        
        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            # Submit all file processing tasks
            future_to_file = {
                executor.submit(self._process_single_summary_file, file_path): file_path 
                for file_path in files
            }
            
            # Collect results as they complete
            for future in as_completed(future_to_file):
                file_path = future_to_file[future]
                try:
                    file_records = future.result()
                    all_records.extend(file_records)
                    logger.debug(f"Processed summary file: {file_path} ({len(file_records)} records)")
                except Exception as e:
                    logger.error(f"Thread failed processing summary file {file_path}: {e}")
        
        logger.info(f"Successfully loaded {len(all_records)} summary records from {len(files)} files")
        return pd.DataFrame(all_records)
    
    def load_streaming_data(self, json_files: List[Path], csv_files: List[Path]) -> pd.DataFrame:
        """Load and process streaming data using threading for improved performance."""
        if not json_files and not csv_files:
            return pd.DataFrame()
        
        all_files = len(json_files) + len(csv_files)
        logger.info(f"Loading {all_files} streaming files using threading...")
        streaming_records = []
        
        # Use ThreadPoolExecutor to process files concurrently
        max_workers = min(all_files, 6)  # Cap at 6 threads for streaming files (they're larger)
        
        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            # Submit JSON file processing tasks
            future_to_info = {}
            for file_path in json_files:
                future = executor.submit(self._process_single_streaming_json_file, file_path)
                future_to_info[future] = ('json', file_path)
            
            # Submit CSV file processing tasks
            for file_path in csv_files:
                future = executor.submit(self._process_single_streaming_csv_file, file_path)
                future_to_info[future] = ('csv', file_path)
            
            # Collect results as they complete
            for future in as_completed(future_to_info):
                file_type, file_path = future_to_info[future]
                try:
                    df = future.result()
                    if not df.empty:
                        # Add metadata for tracking
                        df['file_type'] = f'streaming_{file_type}'
                        streaming_records.append(df)
                        logger.debug(f"Processed streaming {file_type} file: {file_path} ({len(df)} records)")
                    else:
                        logger.debug(f"Empty result from streaming {file_type} file: {file_path}")
                except Exception as e:
                    logger.error(f"Thread failed processing streaming {file_type} file {file_path}: {e}")
        
        # Combine all DataFrames
        if streaming_records:
            combined_df = pd.concat(streaming_records, ignore_index=True)
            logger.info(f"Successfully loaded {len(combined_df)} streaming records from {len(streaming_records)} files")
            return combined_df
        else:
            logger.warning("No valid streaming data found")
            return pd.DataFrame()
    
    def process_all_data(self) -> Dict:
        """Discover and process all benchmark data files."""
        logger.info(f"Discovering files in {self.results_dir}")
        files = self.discover_files()
        
        logger.info(f"Found {len(files['summary_json'])} summary JSON, "
                   f"{len(files['streaming_json'])} streaming JSON, "
                   f"{len(files['streaming_csv'])} streaming CSV files")
        
        # Load summary data
        if files['summary_json']:
            self.data_store['summary_data'] = self.load_summary_data(files['summary_json'])
            logger.info(f"Loaded {len(self.data_store['summary_data'])} summary records")
        
        # Load streaming data
        if files['streaming_json'] or files['streaming_csv']:
            self.data_store['streaming_data'] = self.load_streaming_data(
                files['streaming_json'], files['streaming_csv']
            )
            logger.info(f"Loaded {len(self.data_store['streaming_data'])} streaming records")
        
        return self.data_store

def list_directories(path: str = ".") -> List[Dict]:
    """List directories and relevant files in the given path for the file browser."""
    try:
        # Expand and resolve the path
        resolved_path = Path(os.path.expanduser(path)).resolve()
        
        # Ensure we can read the directory
        if not resolved_path.exists() or not resolved_path.is_dir():
            resolved_path = Path.home()  # Fallback to home directory
            
        items = []
        
        # Add parent directory option (if not at root)
        if resolved_path.parent != resolved_path:
            items.append({
                'name': '.. (Parent Directory)',
                'path': str(resolved_path.parent),
                'is_parent': True,
                'is_directory': True,
                'is_file': False,
                'icon': '',
                'type': 'parent'
            })
        
        # List all items (directories and relevant files)
        try:
            # Separate directories and files
            directories = []
            files = []
            
            for item in sorted(resolved_path.iterdir()):
                if item.is_dir() and not item.name.startswith('.'):
                    # Get directory size info
                    try:
                        item_count = len(list(item.iterdir()))
                        size_info = f" ({item_count} items)"
                    except (PermissionError, OSError):
                        size_info = " (Access denied)"
                    
                    directories.append({
                        'name': item.name + size_info,
                        'path': str(item),
                        'is_parent': False,
                        'is_directory': True,
                        'is_file': False,
                        'icon': '',
                        'type': 'directory'
                    })
                
                elif item.is_file() and not item.name.startswith('.'):
                    # Check if it's a relevant data file
                    file_ext = item.suffix.lower()
                    if file_ext in ['.json', '.csv']:
                        # Get file size
                        try:
                            file_size = item.stat().st_size
                            if file_size < 1024:
                                size_info = f" ({file_size}B)"
                            elif file_size < 1024 * 1024:
                                size_info = f" ({file_size//1024}KB)"
                            else:
                                size_info = f" ({file_size//(1024*1024)}MB)"
                        except (PermissionError, OSError):
                            size_info = ""
                        
                        # Determine file type and icon
                        if file_ext == '.json':
                            icon = ''
                            if '_results.json' in item.name:
                                icon = ''  # Summary results
                            elif '_streaming.json' in item.name:
                                icon = ''  # Streaming data
                        else:  # .csv
                            icon = ''
                            
                        files.append({
                            'name': item.name + size_info,
                            'path': str(item),
                            'is_parent': False,
                            'is_directory': False,
                            'is_file': True,
                            'icon': icon,
                            'type': file_ext[1:]  # 'json' or 'csv'
                        })
            
            # Add directories first, then files
            items.extend(directories)
            items.extend(files)
            
        except (PermissionError, OSError) as e:
            logger.warning(f"Cannot read directory {resolved_path}: {e}")
            
        return items
    except Exception as e:
        logger.error(f"Error listing directory contents: {e}")
        return []

def get_directory_breadcrumbs(path: str) -> List[Dict]:
    """Generate breadcrumbs for the current directory path."""
    try:
        resolved_path = Path(os.path.expanduser(path)).resolve()
        breadcrumbs = []
        
        # Add home as the root
        breadcrumbs.append({
            'name': 'Home',
            'path': str(Path.home())
        })
        
        # If not in home directory, add path components
        if resolved_path != Path.home():
            try:
                # Get relative path from home
                rel_path = resolved_path.relative_to(Path.home())
                current_path = Path.home()
                
                for part in rel_path.parts:
                    current_path = current_path / part
                    breadcrumbs.append({
                        'name': part,
                        'path': str(current_path)
                    })
            except ValueError:
                # Path is not under home, show absolute path components
                breadcrumbs = [{'name': 'Root', 'path': '/'}]
                current_path = Path('/')
                
                for part in resolved_path.parts[1:]:  # Skip empty root part
                    current_path = current_path / part
                    breadcrumbs.append({
                        'name': part,
                        'path': str(current_path)
                    })
                    
        return breadcrumbs
    except Exception as e:
        logger.error(f"Error generating breadcrumbs: {e}")
        return [{'name': 'Home', 'path': str(Path.home())}]
