#!/usr/bin/env python3
"""
Interactive Dashboard for Rusty-Comms Benchmark Results

A comprehensive Plotly Dash application for visualizing IPC benchmark results
from both JSON and CSV output formats with interactive filtering and analysis.
"""

import argparse
import json
import logging
import pandas as pd
import plotly.express as px
import plotly.graph_objects as go
from plotly.subplots import make_subplots
import dash
from dash import dcc, html, Input, Output, State, callback, dash_table
from dash.exceptions import PreventUpdate
import numpy as np
from concurrent.futures import ThreadPoolExecutor, as_completed
import threading

from scipy import stats
try:
    from sklearn.ensemble import IsolationForest
    SKLEARN_AVAILABLE = True
except ImportError:
    SKLEARN_AVAILABLE = False
from pathlib import Path
from typing import Dict, List, Tuple, Optional, Union

# Configure logging
logging.basicConfig(level=logging.INFO)
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

class DashboardApp:
    """Main Dash application for interactive benchmark visualization."""
    
    def __init__(self, data_store: Dict, initial_dir: str = "."):
        self.data_store = data_store
        self.initial_dir = initial_dir
        self.app = dash.Dash(__name__)
        # Inject modern CSS
        self.app.index_string = '''
        <!DOCTYPE html>
        <html>
            <head>
                {%metas%}
                <title>{%title%}</title>
                {%favicon%}
                {%css%}
                <style>
                ''' + app_css + '''
                </style>
            </head>
            <body>
                {%app_entry%}
                <footer>
                    {%config%}
                    {%scripts%}
                    {%renderer%}
                </footer>
            </body>
        </html>
        '''
        self.setup_layout()
        self.setup_callbacks()
    
    def reload_data(self, directory_path: str) -> tuple:
        """Reload data from a new directory and update the data store."""
        try:
            from pathlib import Path
            
            # Validate directory path
            dir_path = Path(directory_path)
            if not dir_path.exists():
                return False, f"Error: Directory '{directory_path}' does not exist."
            
            if not dir_path.is_dir():
                return False, f"Error: '{directory_path}' is not a directory."
            
            # Load data from new directory
            logger.info(f"Reloading data from directory: {directory_path}")
            data_loader = BenchmarkDataProcessor(dir_path)
            new_data_store = data_loader.process_all_data()
            
            # Check if any data was found
            summary_count = len(new_data_store['summary_data'])
            streaming_count = len(new_data_store['streaming_data'])
            
            if summary_count == 0 and streaming_count == 0:
                return False, f"Warning: No benchmark data files found in '{directory_path}'."
            
            # Update the data store
            self.data_store.update(new_data_store)
            
            # Success message
            success_msg = f"Success: Loaded {summary_count} summary records and {streaming_count:,} streaming records from '{directory_path}'."
            logger.info(success_msg)
            
            return True, success_msg
            
        except Exception as e:
            error_msg = f"Error loading data: {str(e)}"
            logger.error(error_msg)
            return False, error_msg
    
    def get_filter_options(self):
        """Extract unique values for filter controls."""
        summary_df = self.data_store['summary_data']
        streaming_df = self.data_store['streaming_data']
        
        # Combine mechanisms from both datasets
        mechanisms = set()
        message_sizes = set()
        
        if not summary_df.empty:
            mechanisms.update(summary_df['mechanism'].unique())
            # Filter out 0 and invalid message sizes
            valid_message_sizes = [x for x in summary_df['message_size'].unique() if x > 0]
            message_sizes.update(valid_message_sizes)
        
        if not streaming_df.empty:
            mechanisms.update(streaming_df['mechanism'].unique())
            # Filter out 0 and invalid message sizes
            valid_message_sizes = [x for x in streaming_df['message_size'].unique() if x > 0]
            message_sizes.update(valid_message_sizes)
        
        result = {
            'mechanisms': sorted(list(mechanisms)),
            'message_sizes': sorted(list(message_sizes))
        }
        
        logger.info(f"Filter options: {result}")
        return result
    
    def setup_layout(self):
        """Setup the main dashboard layout."""
        filter_options = self.get_filter_options()
        
        self.app.layout = html.Div(children=[
            html.Div([
                # Sidebar Controls
                html.Div([
                    html.H1("Rusty-Comms Dashboard", className="header-title"),
                    
                    # Data Directory Controls
                    html.Div([
                        html.H3("Data Directory"),
                        html.P("Select a directory containing benchmark data files.", 
                               style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '10px', 'font-style': 'italic'}),
                        
                        html.Div([
                            dcc.Dropdown(
                                id='data-directory-dropdown',
                                options=[
                                    {'label': 'Current Directory (.)', 'value': '.'},
                                    {'label': 'Parent Directory (..)', 'value': '..'},
                                    {'label': 'Home Directory (~)', 'value': '~'},
                                    {'label': 'Desktop (~/Desktop)', 'value': '~/Desktop'},
                                    {'label': 'Downloads (~/Downloads)', 'value': '~/Downloads'},
                                    {'label': 'Documents (~/Documents)', 'value': '~/Documents'},
                                    {'label': 'Custom Path...', 'value': 'custom'}
                                ],
                                value='.',
                                clearable=False,
                                style={'margin-bottom': '10px'}
                            ),
                            
                            # Custom path input (hidden by default)
                            html.Div([
                                dcc.Input(
                                    id='custom-directory-input',
                                    type='text',
                                    placeholder='Enter custom directory path...',
                                    value='',
                                    style={
                                        'width': '100%', 
                                        'padding': '8px', 
                                        'border': '1px solid #d1d5db',
                                        'border-radius': '4px',
                                        'font-size': '0.9rem'
                                    }
                                )
                            ], id='custom-path-container', style={'display': 'none', 'margin-bottom': '10px'}),
                            
                            html.Button(
                                'Reload Data', 
                                id='reload-data-button',
                                n_clicks=0,
                                style={
                                    'width': '100%',
                                    'padding': '10px 16px',
                                    'background-color': '#3b82f6',
                                    'color': 'white',
                                    'border': 'none',
                                    'border-radius': '6px',
                                    'cursor': 'pointer',
                                    'font-size': '0.95rem',
                                    'font-weight': '600'
                                }
                            ),
                        ], style={'margin-bottom': '20px'}),
                        
                        # Status message for reload feedback
                        html.Div(id='reload-status', style={'margin-bottom': '15px'})
                    ], className="filter-section"),
                    
                    html.Div([
                        html.H3("Filters"),
                        html.P("Select items to filter data. All options are selected by default.", 
                               style={'color': '#6b7280', 'font-size': '0.9rem', 'margin-bottom': '15px', 'font-style': 'italic'}),
                        
                        html.Label("Mechanisms:"),
                        dcc.Checklist(
                            id='mechanism-filter',
                            options=[{'label': m, 'value': m} for m in filter_options['mechanisms']],
                            value=filter_options['mechanisms'],  # Start with all mechanisms selected
                            className="filter-checklist"
                        ),
                    ], className="filter-section"),
                    
                    html.Div([
                        html.Label("Message Sizes:"),
                        dcc.Checklist(
                            id='message-size-filter',
                            options=[{'label': f"{s}B", 'value': s} for s in filter_options['message_sizes']],
                            value=filter_options['message_sizes'],  # Start with all message sizes selected
                            className="filter-checklist"
                        ),
                    ], className="filter-section"),
                    

                    

                    

                    
                ], className="sidebar"),
                
                # Main Content
                html.Div([
                    dcc.Tabs(
                        id="main-tabs", 
                        value="summary-tab",
                        style={
                            'height': '50px',
                            'marginBottom': '25px'
                        },
                        children=[
                            dcc.Tab(
                                label="Summary", 
                                value="summary-tab",
                                style={'padding': '12px 24px', 'fontWeight': '500'},
                                selected_style={'padding': '12px 24px', 'fontWeight': '600', 'color': '#667eea'}
                            ),
                            dcc.Tab(
                                label="Time Series", 
                                value="timeseries-tab",
                                style={'padding': '12px 24px', 'fontWeight': '500'},
                                selected_style={'padding': '12px 24px', 'fontWeight': '600', 'color': '#667eea'}
                            ),

                        ]
                    ),
                    html.Div(id="tab-content", className="tab-content")
                ], className="main-content")
                
            ], className="dashboard-container")
        ], style={'background': 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)', 'minHeight': '100vh'})
    
    def setup_callbacks(self):
        """Setup all dashboard callbacks."""
        
        @self.app.callback(
            Output("tab-content", "children"),
            [Input("main-tabs", "value"),
             Input("mechanism-filter", "value"),
             Input("message-size-filter", "value")],
            prevent_initial_call=False
        )
        def render_tab_content(active_tab, mechanisms, message_sizes):
            if active_tab == "summary-tab":
                logger.info(f"Summary tab called with filters: mechanisms={mechanisms}, message_sizes={message_sizes}")
                return self.render_summary_tab(mechanisms, message_sizes)
            elif active_tab == "timeseries-tab":
                return self.render_timeseries_tab(mechanisms, message_sizes)
            return html.Div("Select a tab")
        


        # Enhanced time series charts update callback with all new controls
        @self.app.callback(
            Output('ts-charts-container', 'children'),
            [Input('ts-moving-avg-slider', 'value'),
             Input('ts-display-options', 'value'),
             Input('ts-chart-layout', 'value'),
             Input('ts-view-options', 'value'),
             Input('ts-latency-types', 'value'),
             Input('ts-statistical-overlays', 'value'),
             Input('ts-y-axis-scale', 'value'),
             Input('ts-y-axis-range', 'value'),
             Input('ts-sampling-strategy', 'value'),
             Input("mechanism-filter", "value"),
             Input("message-size-filter", "value")],
            prevent_initial_call=True
        )
        def update_timeseries_charts(moving_avg_window, display_options, 
                                   chart_layout, view_options, latency_types, statistical_overlays,
                                   y_axis_scale, y_axis_range, sampling_strategy,
                                   mechanisms, message_sizes):
            from dash import ctx
            
            # Handle None values for controls with defaults
            chart_layout = chart_layout if chart_layout is not None else 'separate'
            view_options = view_options if view_options is not None else ['sync_zoom']
            latency_types = latency_types if latency_types is not None else ['one_way']
            statistical_overlays = statistical_overlays if statistical_overlays is not None else ['percentile_bands']
            y_axis_scale = y_axis_scale if y_axis_scale is not None else 'log'
            y_axis_range = y_axis_range if y_axis_range is not None else 'auto'
            sampling_strategy = sampling_strategy if sampling_strategy is not None else 'uniform'
            

            
            return self.render_timeseries_charts(
                mechanisms, message_sizes, moving_avg_window, display_options, 
                chart_layout, view_options, latency_types, statistical_overlays, 
                y_axis_scale, y_axis_range, sampling_strategy
            )
        


        # Real-time statistics panel update callback
        @self.app.callback(
            Output('ts-stats-panel', 'children'),
            [Input('ts-view-options', 'value'),
             Input('ts-latency-types', 'value'),
             Input("mechanism-filter", "value"),
             Input("message-size-filter", "value")],
            prevent_initial_call=False
        )
        def update_stats_panel(view_options, latency_types, mechanisms, message_sizes):
            if not view_options or 'show_stats' not in view_options:
                return html.Div()
            
            if not latency_types:
                latency_types = ['one_way']  # Default to one-way if nothing selected
            
            # Generate statistics overview
            streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
            
            if streaming_df.empty:
                return html.Div()
            
            # Create separate tables for each mechanism
            mechanism_tables = []
            for mechanism in sorted(streaming_df['mechanism'].unique()):
                mechanism_data = streaming_df[streaming_df['mechanism'] == mechanism]
                
                # Create stats for each latency type selected
                for latency_type in latency_types:
                    latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                    type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                    
                    # Skip if column doesn't exist or has no data
                    if latency_col not in mechanism_data.columns or mechanism_data[latency_col].isna().all():
                        continue
                    
                    stats_data = []
                    for msg_size in sorted(mechanism_data['message_size'].unique()):
                        subset_data = mechanism_data[mechanism_data['message_size'] == msg_size]
                        if len(subset_data) > 0 and not subset_data[latency_col].isna().all():
                            stats_data.append({
                                'Message Size (bytes)': f"{msg_size:,}",
                                'Count': f"{len(subset_data):,}",
                                'Mean (μs)': f"{subset_data[latency_col].mean():.2f}",
                                'Std (μs)': f"{subset_data[latency_col].std():.2f}",
                                'Min (μs)': f"{subset_data[latency_col].min():.2f}",
                                'Max (μs)': f"{subset_data[latency_col].max():.2f}",
                                'P95 (μs)': f"{subset_data[latency_col].quantile(0.95):.2f}",
                                'P99 (μs)': f"{subset_data[latency_col].quantile(0.99):.2f}"
                            })
                    
                    if stats_data:
                        # Color scheme for each mechanism
                        mechanism_colors = {
                            'PosixMessageQueue': 'rgba(59, 130, 246, 0.1)',
                            'SharedMemory': 'rgba(34, 197, 94, 0.1)', 
                            'TcpSocket': 'rgba(251, 146, 60, 0.1)',
                            'UnixDomainSocket': 'rgba(168, 85, 247, 0.1)'
                        }
                        
                        table = html.Div([
                            html.H5(f"{mechanism} - {type_label} Latency", 
                                   style={'margin-bottom': '10px', 'color': '#1f2937', 'font-weight': 'bold'}),
                            dash_table.DataTable(
                                data=stats_data,
                                columns=[{"name": col, "id": col} for col in stats_data[0].keys()],
                                style_cell={'textAlign': 'left', 'padding': '8px', 'font-size': '0.8rem'},
                                style_header={'backgroundColor': '#f8fafc', 'fontWeight': 'bold', 'border': '1px solid #e2e8f0'},
                                style_data={'backgroundColor': mechanism_colors.get(mechanism, '#ffffff'), 'border': '1px solid #e2e8f0'},
                                style_table={'border-radius': '8px', 'overflow': 'hidden'},
                                style_cell_conditional=[
                                    {'if': {'column_id': 'Message Size (bytes)'}, 'textAlign': 'right', 'font-weight': 'bold'},
                                    {'if': {'column_id': 'Count'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'Mean (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'Std (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'Min (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'Max (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'P95 (μs)'}, 'textAlign': 'right'},
                                    {'if': {'column_id': 'P99 (μs)'}, 'textAlign': 'right'}
                                ]
                            )
                        ], style={'margin-bottom': '20px'})
                        mechanism_tables.append(table)
            
            if not mechanism_tables:
                return html.Div()
            
            return html.Div([
                html.H4("Statistics Overview", style={'margin-bottom': '20px', 'color': '#1f2937', 'font-size': '1.1rem'}),
                html.Div(mechanism_tables)
            ], style={'margin-bottom': '30px', 'padding': '20px', 'background': '#f8fafc', 'border-radius': '8px', 'border': '1px solid #e2e8f0'})
        
        # Preset configuration buttons callbacks
        @self.app.callback(
            [Output('ts-chart-layout', 'value'),
             Output('ts-view-options', 'value'),
             Output('ts-statistical-overlays', 'value'),
             Output('ts-y-axis-scale', 'value'),
             Output('ts-sampling-strategy', 'value'),
             Output('ts-display-options', 'value'),
             Output('ts-latency-types', 'value')],
            [Input('preset-performance', 'n_clicks'),
             Input('preset-detailed', 'n_clicks'),
             Input('preset-statistical', 'n_clicks'),
             Input('preset-outliers', 'n_clicks'),
             Input('preset-reset', 'n_clicks')],
            [State('ts-view-options', 'value')],
            prevent_initial_call=True
        )
        def handle_preset_buttons(perf_clicks, detailed_clicks, stat_clicks, outlier_clicks, reset_clicks, current_view_options):
            import dash
            from dash import ctx
            
            if not ctx.triggered:
                raise PreventUpdate
            
            button_id = ctx.triggered[0]['prop_id'].split('.')[0]
            
            # Preserve the "show_stats" setting from current view options
            current_view_options = current_view_options or []
            preserve_stats = 'show_stats' in current_view_options
            
            def build_view_options(base_options):
                """Helper to build view options while preserving stats panel setting"""
                result = base_options.copy()
                # Force disable show_stats for all presets
                if 'show_stats' in result:
                    result.remove('show_stats')
                return result
            
            if button_id == 'preset-performance':
                # Performance Analysis preset
                return ('separate', build_view_options(['sync_zoom']), 
                       ['spike_detection'], 
                       'log', 'uniform', ['moving_avg'], ['round_trip'])
            
            elif button_id == 'preset-detailed':
                # Detailed Inspection preset
                return ('faceted', build_view_options(['sync_zoom']), 
                       [], 
                       'log', 'uniform', ['moving_avg'], ['round_trip'])
            
            elif button_id == 'preset-statistical':
                # Statistical Overview preset
                return ('faceted', build_view_options(['sync_zoom']), 
                       ['percentile_bands', 'spike_detection'], 
                       'log', 'peak_preserving', ['moving_avg'], ['one_way', 'round_trip'])
            
            elif button_id == 'preset-outliers':
                # Outlier Detection preset
                return ('separate', build_view_options(['sync_zoom']), 
                       ['percentile_bands', 'spike_detection', 'anomaly_detection'], 
                       'log', 'outlier_preserving', ['moving_avg'], ['round_trip'])
            
            elif button_id == 'preset-reset':
                # Reset to defaults (but preserve statistics panel setting)
                return ('separate', build_view_options(['sync_zoom']), 
                       ['percentile_bands', 'spike_detection'], 
                       'log', 'uniform', ['raw_dots', 'moving_avg'], ['one_way'])
            
            raise PreventUpdate
        
        # Directory dropdown callback to show/hide custom input
        @self.app.callback(
            Output('custom-path-container', 'style'),
            [Input('data-directory-dropdown', 'value')],
            prevent_initial_call=False
        )
        def toggle_custom_input(dropdown_value):
            if dropdown_value == 'custom':
                return {'display': 'block', 'margin-bottom': '10px'}
            else:
                return {'display': 'none', 'margin-bottom': '10px'}
        
        # Data reload callback
        @self.app.callback(
            [Output('reload-status', 'children'),
             Output('mechanism-filter', 'options'),
             Output('mechanism-filter', 'value'),
             Output('message-size-filter', 'options'),
             Output('message-size-filter', 'value')],
            [Input('reload-data-button', 'n_clicks')],
            [State('data-directory-dropdown', 'value'),
             State('custom-directory-input', 'value')],
            prevent_initial_call=True
        )
        def handle_data_reload(n_clicks, dropdown_value, custom_path):
            if n_clicks == 0:
                # No callback needed for initial state
                raise PreventUpdate
            
            # Determine the directory path to use
            if dropdown_value == 'custom':
                directory_path = custom_path if custom_path else '.'
            else:
                directory_path = dropdown_value
            
            # Expand ~ for home directory
            if directory_path.startswith('~'):
                import os
                directory_path = os.path.expanduser(directory_path)
            
            # Attempt to reload data
            success, message = self.reload_data(directory_path)
            
            if success:
                # Update filter options with new data
                new_filter_options = self.get_filter_options()
                
                # Create status message with success styling
                status_div = html.Div([
                    html.P(message, style={
                        'color': '#059669', 
                        'font-size': '0.85rem', 
                        'margin': '8px 0',
                        'padding': '8px 12px',
                        'background-color': '#d1fae5',
                        'border': '1px solid #10b981',
                        'border-radius': '4px'
                    })
                ])
                
                # Return updated filter options and select all by default
                return (
                    status_div,
                    [{'label': m, 'value': m} for m in new_filter_options['mechanisms']],
                    new_filter_options['mechanisms'],  # Select all mechanisms by default
                    [{'label': str(s), 'value': s} for s in new_filter_options['message_sizes']],
                    new_filter_options['message_sizes']   # Select all message sizes by default
                )
            else:
                # Create status message with error styling
                status_div = html.Div([
                    html.P(message, style={
                        'color': '#dc2626', 
                        'font-size': '0.85rem', 
                        'margin': '8px 0',
                        'padding': '8px 12px',
                        'background-color': '#fef2f2',
                        'border': '1px solid #f87171',
                        'border-radius': '4px'
                    })
                ])
                
                # Keep existing filter options on error and maintain all selections
                current_filter_options = self.get_filter_options()
                return (
                    status_div,
                    [{'label': m, 'value': m} for m in current_filter_options['mechanisms']],
                    current_filter_options['mechanisms'],  # Keep all mechanisms selected
                    [{'label': str(s), 'value': s} for s in current_filter_options['message_sizes']],
                    current_filter_options['message_sizes']   # Keep all message sizes selected
                )

    def filter_data(self, df: pd.DataFrame, mechanisms: List, message_sizes: List) -> pd.DataFrame:
        """Apply filters to dataframe. Empty filter lists mean 'show all' for that dimension."""
        if df.empty:
            return df
        
        filtered = df.copy()
        
        # Apply filters only if selections are made. Empty list = show all data for that dimension
        if mechanisms is not None and len(mechanisms) > 0:
            filtered = filtered[filtered['mechanism'].isin(mechanisms)]
        # If mechanisms is None or empty, show all mechanisms
        
        if message_sizes is not None and len(message_sizes) > 0:
            filtered = filtered[filtered['message_size'].isin(message_sizes)]
        # If message_sizes is None or empty, show all message sizes
        
        return filtered
    
    def generate_performance_insights(self, percentile_df: pd.DataFrame, throughput_df: pd.DataFrame = None, max_latency_df: pd.DataFrame = None) -> Dict:
        """Generate mechanism + message size specific performance recommendations based on lowest jitter and worst-case latency."""
        insights = {
            'best_mechanism': None,
            'best_latency': None,
            'best_max_latency': None,
            'best_max_mechanism': None,
            'best_consistency_mechanism': None,
            'best_throughput': None,
            'recommendations': [],
            'summary_stats': {}
        }
        
        if percentile_df.empty:
            insights['recommendations'].append("**NO DATA**: Select mechanisms and message sizes to generate performance insights")
            return insights
        
        # Get one-way latency data for analysis
        one_way_p50 = percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P50 (Median)')
        ]
        one_way_p95 = percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P95')
        ]
        
        if one_way_p50.empty or max_latency_df is None or max_latency_df.empty:
            insights['recommendations'].append("**INSUFFICIENT DATA**: Need P50, P95, and max latency data for analysis")
            return insights
        
        one_way_max = max_latency_df[max_latency_df['latency_type'] == 'One-way']
        if one_way_max.empty:
            insights['recommendations'].append("**INSUFFICIENT DATA**: Need max latency data for analysis")
            return insights
        
        # Calculate performance scores for each mechanism + message size combination
        performance_scores = []
        
        for message_size in sorted(one_way_p50['message_size'].unique()):
            size_p50 = one_way_p50[one_way_p50['message_size'] == message_size]
            size_p95 = one_way_p95[one_way_p95['message_size'] == message_size]
            size_max = one_way_max[one_way_max['message_size'] == message_size]
            
            for mechanism in size_p50['mechanism'].unique():
                p50_data = size_p50[size_p50['mechanism'] == mechanism]['latency_us']
                p95_data = size_p95[size_p95['mechanism'] == mechanism]['latency_us']
                max_data = size_max[size_max['mechanism'] == mechanism]['max_latency_us']
                
                if not p50_data.empty and not p95_data.empty and not max_data.empty:
                    p50_val = p50_data.iloc[0]
                    p95_val = p95_data.iloc[0]
                    max_val = max_data.iloc[0]
                    
                    # Calculate jitter (P95-P50 spread) - lower is better
                    jitter = p95_val - p50_val
                    
                    # Combined score: normalize both jitter and max latency, then combine
                    # Lower scores are better
                    performance_scores.append({
                        'mechanism': mechanism,
                        'message_size': message_size,
                        'p50': p50_val,
                        'p95': p95_val,
                        'max': max_val,
                        'jitter': jitter,
                        'score': max_val + jitter  # Simple combination: max latency + jitter penalty
                    })
        
        if not performance_scores:
            insights['recommendations'].append("**NO DATA**: Unable to calculate performance scores")
            return insights
        
        # Generate recommendations by message size
        message_sizes = sorted(set(score['message_size'] for score in performance_scores))
        
        for message_size in message_sizes:
            size_scores = [s for s in performance_scores if s['message_size'] == message_size]
            if len(size_scores) <= 1:
                continue
                
            # Sort by combined score (lower is better)
            size_scores.sort(key=lambda x: x['score'])
            
            best = size_scores[0]
            worst = size_scores[-1]
            
            # Create recommendation
            if len(size_scores) == 2:
                # Simple comparison between two mechanisms
                score_improvement = ((worst['score'] - best['score']) / worst['score']) * 100
                insights['recommendations'].append(
                    f"**{message_size}B**: {best['mechanism']} excels with {best['jitter']:.1f}μs jitter + {best['max']:.1f}μs max latency ({score_improvement:.0f}% better than {worst['mechanism']})"
                )
            else:
                # Multiple mechanisms - show best and mention others
                runner_up = size_scores[1]
                insights['recommendations'].append(
                    f"**{message_size}B**: {best['mechanism']} leads with {best['jitter']:.1f}μs jitter + {best['max']:.1f}μs max latency (beats {runner_up['mechanism']} and {len(size_scores)-2} others)"
                )
        
        # Overall analysis: find the mechanism that appears most often as winner
        mechanism_wins = {}
        for message_size in message_sizes:
            size_scores = [s for s in performance_scores if s['message_size'] == message_size]
            if size_scores:
                size_scores.sort(key=lambda x: x['score'])
                winner = size_scores[0]['mechanism']
                mechanism_wins[winner] = mechanism_wins.get(winner, 0) + 1
        
        if mechanism_wins:
            overall_winner = max(mechanism_wins, key=mechanism_wins.get)
            win_count = mechanism_wins[overall_winner]
            total_sizes = len(message_sizes)
            
            if win_count == total_sizes:
                insights['recommendations'].insert(0, f"**CHAMPION**: {overall_winner} dominates across ALL {total_sizes} message sizes with optimal jitter + max latency")
            elif win_count > total_sizes / 2:
                insights['recommendations'].insert(0, f"**TOP PERFORMER**: {overall_winner} wins {win_count}/{total_sizes} message sizes with superior jitter + max latency control")
            else:
                insights['recommendations'].insert(0, f"**CONTEXT MATTERS**: No single winner - choose {overall_winner} for {win_count} sizes, others context-dependent")
        
        # Add warning for extreme outliers
        extreme_outliers = [s for s in performance_scores if s['max'] > s['p50'] * 20 and s['max'] > 100]
        if extreme_outliers:
            worst_outlier = max(extreme_outliers, key=lambda x: x['max'] / x['p50'])
            ratio = worst_outlier['max'] / worst_outlier['p50']
            insights['recommendations'].insert(0, 
                f"**WARNING**: {worst_outlier['mechanism']} at {worst_outlier['message_size']}B has extreme spikes ({worst_outlier['max']:.0f}μs max vs {worst_outlier['p50']:.1f}μs typical = {ratio:.0f}x worse)"
            )
        
        # Throughput analysis for summary stats
        if throughput_df is not None and not throughput_df.empty:
            one_way_throughput = throughput_df[throughput_df['type'] == 'One-way']
            if not one_way_throughput.empty:
                best_throughput_row = one_way_throughput.loc[one_way_throughput['msgs_per_sec'].idxmax()]
                insights['best_throughput'] = {
                    'mechanism': best_throughput_row['mechanism'],
                    'msgs_per_sec': best_throughput_row['msgs_per_sec'],
                    'message_size': best_throughput_row['message_size']
                }
        
        # Store summary stats for cards
        if performance_scores:
            best_overall = min(performance_scores, key=lambda x: x['score'])
            insights['best_mechanism'] = best_overall['mechanism']
            insights['best_latency'] = best_overall['p50']
            insights['best_max_latency'] = best_overall['max']
            insights['best_max_mechanism'] = best_overall['mechanism']
            insights['summary_stats']['best_p50'] = best_overall['p50']
            insights['summary_stats']['best_max_latency'] = best_overall['max']
        
        return insights
    
    def create_performance_comparison_matrix(self, percentile_df: pd.DataFrame, max_latency_df: pd.DataFrame = None) -> tuple:
        """Create performance comparison matrices showing relative performance for P50 and max latency."""
        p50_matrix = pd.DataFrame()
        max_matrix = pd.DataFrame()
        
        # P50 Latency Matrix
        if not percentile_df.empty:
            # Focus on P50 one-way latency for the matrix
            p50_data = percentile_df[
                (percentile_df['latency_type'] == 'One-way') & 
                (percentile_df['percentile'] == 'P50 (Median)')
            ].copy()
            
            if not p50_data.empty:
                # Create pivot table with mechanisms as rows and message sizes as columns
                p50_matrix = p50_data.pivot(index='mechanism', columns='message_size', values='latency_us')
                
                # Calculate relative performance (percentage compared to best for each message size)
                for col in p50_matrix.columns:
                    col_data = p50_matrix[col].dropna()
                    if not col_data.empty:
                        best_value = col_data.min()
                        p50_matrix[col] = ((col_data - best_value) / best_value * 100).round(1)
                
                # Add overall score (average relative performance)
                p50_matrix['Overall Score'] = p50_matrix.mean(axis=1).round(1)
                p50_matrix = p50_matrix.fillna('-')
        
        # Max Latency Matrix
        if max_latency_df is not None and not max_latency_df.empty:
            # Focus on one-way max latency for the matrix
            max_data = max_latency_df[max_latency_df['latency_type'] == 'One-way'].copy()
            
            if not max_data.empty:
                # Create pivot table with mechanisms as rows and message sizes as columns
                max_matrix = max_data.pivot(index='mechanism', columns='message_size', values='max_latency_us')
                
                # Calculate relative performance (percentage compared to best for each message size)
                for col in max_matrix.columns:
                    col_data = max_matrix[col].dropna()
                    if not col_data.empty:
                        best_value = col_data.min()  # Lower max latency is better
                        max_matrix[col] = ((col_data - best_value) / best_value * 100).round(1)
                
                # Add overall score (average relative performance)
                max_matrix['Overall Score'] = max_matrix.mean(axis=1).round(1)
                max_matrix = max_matrix.fillna('-')
        
        return p50_matrix, max_matrix
    
    def create_summary_cards(self, insights: Dict, percentile_df: pd.DataFrame, max_latency_df: pd.DataFrame = None) -> html.Div:
        """Create summary performance cards for the top of the dashboard."""
        if not insights or percentile_df.empty:
            return html.Div()
        
        # Best Mechanism Card
        best_mechanism_card = html.Div([
            html.Div([
                html.H4("Best Overall", style={'margin': '0', 'color': '#059669', 'font-size': '0.9rem'}),
                html.H2(insights.get('best_mechanism', 'N/A'), style={'margin': '5px 0', 'color': '#065f46'}),
                html.P(f"P50: {insights.get('best_latency', 0):.1f}μs", style={'margin': '0', 'color': '#6b7280', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #d1fae5 0%, #a7f3d0 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #10b981'
            })
        ], style={'flex': '1', 'min-width': '0'})
        
        # Performance Range Card for Best Mechanism
        best_mechanism = insights.get('best_mechanism', 'N/A')
        
        # Get all latency data for the best mechanism (not just P50)
        if best_mechanism != 'N/A' and not percentile_df.empty:
            best_mechanism_data = percentile_df[
                (percentile_df['mechanism'] == best_mechanism) & 
                (percentile_df['latency_type'] == 'One-way')
            ]
            
            if not best_mechanism_data.empty:
                min_latency = best_mechanism_data['latency_us'].min()
                max_latency = best_mechanism_data['latency_us'].max()
                p99_latency = best_mechanism_data[
                    best_mechanism_data['percentile'] == 'P99'
                ]['latency_us'].iloc[0] if len(best_mechanism_data[
                    best_mechanism_data['percentile'] == 'P99'
                ]) > 0 else 0
            else:
                min_latency = max_latency = p99_latency = 0
        else:
            min_latency = max_latency = p99_latency = 0
        
        performance_card = html.Div([
            html.Div([
                html.H4("Performance Range", style={'margin': '0', 'color': '#1e40af', 'font-size': '0.9rem'}),
                html.H2(f"{min_latency:.1f} - {max_latency:.1f}μs", style={'margin': '5px 0', 'color': '#1e3a8a'}),
                html.P(f"{best_mechanism} (P99: {p99_latency:.1f}μs)", style={'margin': '0', 'color': '#6b7280', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #dbeafe 0%, #bfdbfe 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #3b82f6'
            })
        ], style={'flex': '1', 'min-width': '0'})
        
        # Throughput Card
        throughput_info = insights.get('best_throughput', {})
        throughput_card = html.Div([
            html.Div([
                html.H4("Peak Throughput", style={'margin': '0', 'color': '#7c2d12', 'font-size': '0.9rem'}),
                html.H2(f"{throughput_info.get('msgs_per_sec', 0):,.0f}", style={'margin': '5px 0', 'color': '#92400e'}),
                html.P(f"msgs/sec ({throughput_info.get('mechanism', 'N/A')})", style={'margin': '0', 'color': '#6b7280', 'font-size': '0.9rem'})
            ], style={
                'background': 'linear-gradient(135deg, #fef3c7 0%, #fde68a 100%)',
                'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #f59e0b'
            })
        ], style={'flex': '1', 'min-width': '0'})
        
        # Max Latency Card
        max_latency_card = html.Div()
        if max_latency_df is not None and not max_latency_df.empty:
            one_way_max = max_latency_df[max_latency_df['latency_type'] == 'One-way']
            if not one_way_max.empty:
                best_max_latency = one_way_max['max_latency_us'].min()
                best_max_mechanism = one_way_max.loc[one_way_max['max_latency_us'].idxmin(), 'mechanism']
                
                max_latency_card = html.Div([
                    html.Div([
                        html.H4("Best Max Latency", style={'margin': '0', 'color': '#dc2626', 'font-size': '0.9rem'}),
                        html.H2(f"{best_max_latency:.1f}μs", style={'margin': '5px 0', 'color': '#991b1b'}),
                        html.P(f"{best_max_mechanism}", style={'margin': '0', 'color': '#6b7280', 'font-size': '0.9rem'})
                    ], style={
                        'background': 'linear-gradient(135deg, #fee2e2 0%, #fecaca 100%)',
                        'padding': '20px', 'border-radius': '12px', 'text-align': 'center',
                        'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #dc2626'
                    })
                ], style={'flex': '1', 'min-width': '0'})
        
        # Insights Card (adjusted width)
        cards = [best_mechanism_card, performance_card, throughput_card]
        if max_latency_card.children:  # Only add if max latency data exists
            cards.append(max_latency_card)
        
        return html.Div(cards, style={
            'display': 'flex',
            'flex-direction': 'row', 
            'justify-content': 'space-between',
            'align-items': 'stretch',
            'gap': '15px',
            'margin-bottom': '30px',
            'flex-wrap': 'nowrap'  # Prevent wrapping to ensure single row
        })

    def calculate_percentiles_from_streaming(self, mechanisms: List, message_sizes: List):
        """Calculate percentiles dynamically from streaming data."""
        logger.info(f"calculate_percentiles_from_streaming: Original data shape: {self.data_store['streaming_data'].shape}")
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        logger.info(f"calculate_percentiles_from_streaming: Filtered data shape: {streaming_df.shape}")
        
        if streaming_df.empty:
            logger.warning("Filtered streaming data is empty")
            return pd.DataFrame()
        
        # Calculate percentiles for both one-way and round-trip latencies
        percentile_data = []
        
        # Group by mechanism and message size
        for (mechanism, message_size), group in streaming_df.groupby(['mechanism', 'message_size']):
            # Calculate one-way latency percentiles
            if 'one_way_latency_us' in group.columns:
                one_way_latencies = group['one_way_latency_us'].dropna()
                if len(one_way_latencies) > 0:
                    percentile_data.extend([
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'One-way',
                            'percentile': 'P50 (Median)',
                            'latency_us': np.percentile(one_way_latencies, 50),
                            'sample_count': len(one_way_latencies)
                        },
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'One-way',
                            'percentile': 'P95',
                            'latency_us': np.percentile(one_way_latencies, 95),
                            'sample_count': len(one_way_latencies)
                        },
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'One-way',
                            'percentile': 'P99',
                            'latency_us': np.percentile(one_way_latencies, 99),
                            'sample_count': len(one_way_latencies)
                        }
                    ])
            
            # Calculate round-trip latency percentiles
            if 'round_trip_latency_us' in group.columns:
                round_trip_latencies = group['round_trip_latency_us'].dropna()
                if len(round_trip_latencies) > 0:
                    percentile_data.extend([
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'Round-trip',
                            'percentile': 'P50 (Median)',
                            'latency_us': np.percentile(round_trip_latencies, 50),
                            'sample_count': len(round_trip_latencies)
                        },
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'Round-trip',
                            'percentile': 'P95',
                            'latency_us': np.percentile(round_trip_latencies, 95),
                            'sample_count': len(round_trip_latencies)
                        },
                        {
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'latency_type': 'Round-trip',
                            'percentile': 'P99',
                            'latency_us': np.percentile(round_trip_latencies, 99),
                            'sample_count': len(round_trip_latencies)
                        }
                    ])
        
        return pd.DataFrame(percentile_data)

    def calculate_max_latencies(self, mechanisms: List, message_sizes: List):
        """Calculate maximum latencies from streaming data for performance comparison."""
        logger.info(f"calculate_max_latencies: Original data shape: {self.data_store['streaming_data'].shape}")
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        logger.info(f"calculate_max_latencies: Filtered data shape: {streaming_df.shape}")
        
        if streaming_df.empty:
            logger.warning("Filtered streaming data is empty for max latency calculation")
            return pd.DataFrame()
        
        max_latency_data = []
        
        # Group by mechanism and message size
        for (mechanism, message_size), group in streaming_df.groupby(['mechanism', 'message_size']):
            # Calculate one-way maximum latency
            if 'one_way_latency_us' in group.columns:
                one_way_latencies = group['one_way_latency_us'].dropna()
                if len(one_way_latencies) > 0:
                    max_latency_data.append({
                        'mechanism': mechanism,
                        'message_size': message_size,
                        'latency_type': 'One-way',
                        'max_latency_us': one_way_latencies.max(),
                        'sample_count': len(one_way_latencies)
                    })
            
            # Calculate round-trip maximum latency
            if 'round_trip_latency_us' in group.columns:
                round_trip_latencies = group['round_trip_latency_us'].dropna()
                if len(round_trip_latencies) > 0:
                    max_latency_data.append({
                        'mechanism': mechanism,
                        'message_size': message_size,
                        'latency_type': 'Round-trip',
                        'max_latency_us': round_trip_latencies.max(),
                        'sample_count': len(round_trip_latencies)
                    })
        
        return pd.DataFrame(max_latency_data)

    def create_pivot_table(self, data: List[Dict], value_col: str, index_col: str = 'message_size', 
                          columns_col: str = 'mechanism', round_digits: int = 2) -> pd.DataFrame:
        """Create a pivot table with message size as rows and mechanisms as columns."""
        if not data:
            return pd.DataFrame()
        
        df = pd.DataFrame(data)
        if df.empty:
            return pd.DataFrame()
        
        # Create pivot table
        pivot = df.pivot_table(
            values=value_col, 
            index=index_col, 
            columns=columns_col, 
            aggfunc='mean',  # In case of duplicates, take the mean
            fill_value=None
        )
        
        # Round values
        if round_digits > 0:
            pivot = pivot.round(round_digits)
        
        # Reset index to make message_size a column
        pivot = pivot.reset_index()
        
        # Convert column names to strings and clean up
        pivot.columns = [str(col) if col != index_col else 'Message Size (bytes)' for col in pivot.columns]
        pivot.columns.name = None  # Remove the column name
        
        return pivot

    def render_summary_tab(self, mechanisms: List, message_sizes: List):
        """Render enhanced summary statistics tab with performance insights and better visual hierarchy."""
        logger.info(f"render_summary_tab called with: mechanisms={mechanisms}, message_sizes={message_sizes}")
        
        # Calculate percentiles from streaming data (much more robust!)
        percentile_df = self.calculate_percentiles_from_streaming(mechanisms, message_sizes)
        logger.info(f"Percentile df shape: {percentile_df.shape}, empty: {percentile_df.empty}")
        
        if percentile_df.empty:
            logger.warning("Percentile df is empty, returning no data message")
            return html.Div([
                html.H3("No Data Available"),
                html.P("No streaming data available for the selected filters. Try adjusting your filter selections.", 
                       style={'color': '#6b7280', 'fontSize': '1.1rem', 'textAlign': 'center', 'padding': '40px'})
            ], style={'textAlign': 'center', 'padding': '50px'})
        
        # Sort mechanisms by median one-way latency (best performing first)
        mechanism_order = (percentile_df[
            (percentile_df['latency_type'] == 'One-way') & 
            (percentile_df['percentile'] == 'P50 (Median)')
        ].groupby('mechanism')['latency_us'].mean().sort_values().index.tolist())
        
        # Calculate throughput data first for insights generation
        summary_df = self.filter_data(self.data_store['summary_data'], mechanisms, message_sizes)
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        # Sort message sizes from smallest to largest for consistent ordering
        if not streaming_df.empty:
            message_size_order = sorted(streaming_df['message_size'].unique())
        else:
            message_size_order = []
        
        throughput_data = []
        
        # Use summary data if available, otherwise calculate from streaming data
        if not summary_df.empty:
            for _, row in summary_df.iterrows():
                if not pd.isna(row['one_way_msgs_per_sec']):
                    throughput_data.append({
                        'mechanism': row['mechanism'],
                        'message_size': row['message_size'],
                        'type': 'One-way',
                        'msgs_per_sec': row['one_way_msgs_per_sec'],
                        'bytes_per_sec': row['one_way_bytes_per_sec'],
                        'source': 'Summary Data'
                    })
                if not pd.isna(row['round_trip_msgs_per_sec']):
                    throughput_data.append({
                        'mechanism': row['mechanism'],
                        'message_size': row['message_size'],
                        'type': 'Round-trip',
                        'msgs_per_sec': row['round_trip_msgs_per_sec'],
                        'bytes_per_sec': row['round_trip_bytes_per_sec'],
                        'source': 'Summary Data'
                    })
        
        # If no summary throughput data, estimate from streaming data
        if not throughput_data and not streaming_df.empty:
            logger.info("No summary throughput data found, estimating from streaming data...")
            
            # Group by mechanism and message_size to estimate throughput
            for (mechanism, message_size), group in streaming_df.groupby(['mechanism', 'message_size']):
                if len(group) > 10:  # Need sufficient samples
                    # Calculate time window for throughput estimation
                    if 'timestamp_ns' in group.columns:
                        time_span_sec = (group['timestamp_ns'].max() - group['timestamp_ns'].min()) / 1e9
                        if time_span_sec > 0:
                            msgs_per_sec = len(group) / time_span_sec
                            bytes_per_sec = msgs_per_sec * message_size
                            
                            throughput_data.append({
                                'mechanism': mechanism,
                                'message_size': message_size,
                                'type': 'Estimated',
                                'msgs_per_sec': msgs_per_sec,
                                'bytes_per_sec': bytes_per_sec,
                                'source': 'Streaming Data Estimate'
                            })
        
        throughput_df = pd.DataFrame(throughput_data) if throughput_data else pd.DataFrame()
            
            # Filter out invalid throughput data
        if not throughput_df.empty:
            throughput_df = throughput_df.dropna(subset=['msgs_per_sec', 'bytes_per_sec'])
            throughput_df = throughput_df[(throughput_df['msgs_per_sec'] > 0) & (throughput_df['bytes_per_sec'] > 0)]
            
        # Calculate max latency data for comparison
        max_latency_df = self.calculate_max_latencies(mechanisms, message_sizes)
        
        # Generate insights and summary cards
        insights = self.generate_performance_insights(percentile_df, throughput_df, max_latency_df)
        p50_comparison_matrix, max_comparison_matrix = self.create_performance_comparison_matrix(percentile_df, max_latency_df)
        summary_cards = self.create_summary_cards(insights, percentile_df, max_latency_df)
        
        # Create latency analysis data and charts
        latency_data = []
        
        if not streaming_df.empty:
            # Calculate average and max latencies from streaming data
            for (mechanism, message_size), group in streaming_df.groupby(['mechanism', 'message_size']):
                if 'one_way_latency_us' in group.columns:
                    one_way_data = group['one_way_latency_us'].dropna()
                    if len(one_way_data) > 0:
                        latency_data.append({
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'type': 'Average',
                            'latency_us': one_way_data.mean(),
                            'source': 'One-way'
                        })
                        latency_data.append({
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'type': 'Maximum',
                            'latency_us': one_way_data.max(),
                            'source': 'One-way'
                        })
                
                if 'round_trip_latency_us' in group.columns:
                    round_trip_data = group['round_trip_latency_us'].dropna()
                    if len(round_trip_data) > 0:
                        latency_data.append({
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'type': 'Average',
                            'latency_us': round_trip_data.mean(),
                            'source': 'Round-trip'
                        })
                        latency_data.append({
                            'mechanism': mechanism,
                            'message_size': message_size,
                            'type': 'Maximum',
                            'latency_us': round_trip_data.max(),
                            'source': 'Round-trip'
                        })
        
        # Create latency DataFrames and charts
        if latency_data:
            latency_df = pd.DataFrame(latency_data)
            
            # Average latency chart
            avg_latency_fig = px.bar(
                latency_df[latency_df['type'] == 'Average'],
                x='mechanism',
                y='latency_us',
                color='source',
                facet_col='message_size',
                title="Average Latency (μs) by Mechanism and Message Size (Ordered by Performance)",
                labels={'latency_us': 'Average Latency (μs)', 'message_size': 'Message Size (bytes)'},
                category_orders={'mechanism': mechanism_order, 'message_size': message_size_order},
                color_discrete_map={'One-way': '#10b981', 'Round-trip': '#8b5cf6'}
            )
            # Update facet labels to show only the message size value
            avg_latency_fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
            
            # Maximum latency chart
            max_latency_fig = px.bar(
                latency_df[latency_df['type'] == 'Maximum'],
                x='mechanism',
                y='latency_us',
                color='source',
                facet_col='message_size',
                title="Maximum Latency (μs) by Mechanism and Message Size (Ordered by Performance)",
                labels={'latency_us': 'Maximum Latency (μs)', 'message_size': 'Message Size (bytes)'},
                category_orders={'mechanism': mechanism_order, 'message_size': message_size_order},
                color_discrete_map={'One-way': '#10b981', 'Round-trip': '#8b5cf6'}
            )
            # Update facet labels to show only the message size value
            max_latency_fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
        else:
            avg_latency_fig = go.Figure().add_annotation(
                text="No latency data available",
                x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
                font=dict(size=16, color="#6b7280")
            )
            max_latency_fig = go.Figure().add_annotation(
                text="No latency data available",
                x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
                font=dict(size=16, color="#6b7280")
            )
        
        # Generate throughput charts
        if not throughput_df.empty:
            msgs_fig = px.bar(
                throughput_df,
                x='mechanism',
                y='msgs_per_sec',
                color='type',
                facet_col='message_size',
                title="Throughput (Messages/sec) by Mechanism and Message Size (Ordered by Performance)",
                labels={'msgs_per_sec': 'Messages per Second', 'message_size': 'Message Size (bytes)'},
                category_orders={'mechanism': mechanism_order, 'message_size': message_size_order},
                color_discrete_map={'One-way': '#10b981', 'Round-trip': '#8b5cf6'}
            )
            # Update facet labels to show only the message size value
            msgs_fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
                
            bytes_fig = px.bar(
                throughput_df,
                x='mechanism',
                y='bytes_per_sec',
                color='type',
                facet_col='message_size',
                title="Throughput (Bytes/sec) by Mechanism and Message Size (Ordered by Performance)",
                labels={'bytes_per_sec': 'Bytes per Second', 'message_size': 'Message Size (bytes)'},
                category_orders={'mechanism': mechanism_order, 'message_size': message_size_order},
                color_discrete_map={'One-way': '#10b981', 'Round-trip': '#8b5cf6'}
            )
            # Update facet labels to show only the message size value
            bytes_fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
        else:
            msgs_fig = go.Figure().add_annotation(
                text="No throughput data available",
                x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
                font=dict(size=16, color="#6b7280")
            )
            bytes_fig = go.Figure().add_annotation(
                text="No throughput data available",
                x=0.5, y=0.5, xref="paper", yref="paper", showarrow=False,
                font=dict(size=16, color="#6b7280")
            )
        
        # Add Percentile Statistics Table
        percentile_table_div = html.Div()
        if not percentile_df.empty:
            # Prepare percentile data for display and sort by mechanism performance
            display_percentiles = percentile_df.copy()
            display_percentiles['mechanism'] = pd.Categorical(display_percentiles['mechanism'], categories=mechanism_order, ordered=True)
            display_percentiles = display_percentiles.sort_values(['mechanism', 'message_size', 'latency_type', 'percentile'])
            
            # Round latency values for better display
            display_percentiles['latency_us'] = display_percentiles['latency_us'].round(2)
            
            # Rename columns for better display
            column_mapping = {
                'mechanism': 'Mechanism',
                'message_size': 'Message Size (bytes)',
                'latency_type': 'Latency Type',
                'percentile': 'Percentile',
                'latency_us': 'Latency (μs)',
                'sample_count': 'Sample Count'
            }
            display_percentiles = display_percentiles.rename(columns=column_mapping)
            
            percentile_table = dash_table.DataTable(
                id='percentile-table',
                data=display_percentiles.to_dict('records'),
                columns=[{"name": i, "id": i} for i in display_percentiles.columns],
                style_table={'overflowX': 'auto'},
                style_cell={
                    'textAlign': 'left',
                    'padding': '10px',
                    'fontFamily': 'Arial, sans-serif',
                    'fontSize': '14px'
                },
                style_header={
                    'backgroundColor': '#f8f9fa',
                    'fontWeight': 'bold',
                    'color': '#495057'
                },
                style_data_conditional=[
                    {
                        'if': {'row_index': 'odd'},
                        'backgroundColor': '#f8f9fa'
                    }
                ],
                sort_action="native",
                filter_action="native",
                page_action="native",
                page_current=0,
                page_size=10,
            )
            
            percentile_table_div = html.Div([
                html.H3("Dynamic Percentile Statistics"),
                html.P(f"Percentiles calculated directly from {sum(display_percentiles['Sample Count'])//6:,} streaming data points. "
                       f"This provides accurate, real-time percentile calculations based on your selected filters.",
                       style={'color': '#6b7280', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                percentile_table
            ], className="dash-table-container")

        # Create separate tables for each section
        latency_table_div = html.Div()
        throughput_table_div = html.Div()
        
        if latency_data:
            # Create 4 separate latency tables: Max (one-way/round-trip) and Average (one-way/round-trip)
            latency_df = pd.DataFrame(latency_data)
            
            # Filter data for each table type
            max_oneway_data = latency_df[(latency_df['type'] == 'Maximum') & (latency_df['source'] == 'One-way')]
            max_roundtrip_data = latency_df[(latency_df['type'] == 'Maximum') & (latency_df['source'] == 'Round-trip')]
            avg_oneway_data = latency_df[(latency_df['type'] == 'Average') & (latency_df['source'] == 'One-way')]
            avg_roundtrip_data = latency_df[(latency_df['type'] == 'Average') & (latency_df['source'] == 'Round-trip')]
            
            # Create pivot tables
            max_oneway_pivot = self.create_pivot_table(max_oneway_data.to_dict('records'), 'latency_us')
            max_roundtrip_pivot = self.create_pivot_table(max_roundtrip_data.to_dict('records'), 'latency_us')
            avg_oneway_pivot = self.create_pivot_table(avg_oneway_data.to_dict('records'), 'latency_us')
            avg_roundtrip_pivot = self.create_pivot_table(avg_roundtrip_data.to_dict('records'), 'latency_us')
            
            # Common table style
            table_style = {
                'style_table': {'overflowX': 'auto', 'width': '100%'},
                'style_cell': {'textAlign': 'center', 'padding': '8px', 'fontFamily': 'Arial, sans-serif', 'fontSize': '12px'},
                'style_header': {'backgroundColor': '#f8f9fa', 'fontWeight': 'bold', 'color': '#495057'},
                'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#f8f9fa'}],
                'sort_action': "native"
            }
            
            # Create individual table components
            tables = []
            
            # Max Latency Tables (side by side)
            if not max_oneway_pivot.empty or not max_roundtrip_pivot.empty:
                max_tables_row = html.Div([
                    html.H4("Maximum Latency (μs)", style={'color': '#dc2626', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way max latency table
                        html.Div([
                            html.H5("One-way", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='max-oneway-latency-table',
                                data=max_oneway_pivot.to_dict('records') if not max_oneway_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in max_oneway_pivot.columns] if not max_oneway_pivot.empty else [],
                                **table_style
                            ) if not max_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip max latency table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='max-roundtrip-latency-table',
                                data=max_roundtrip_pivot.to_dict('records') if not max_roundtrip_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in max_roundtrip_pivot.columns] if not max_roundtrip_pivot.empty else [],
                                **table_style
                            ) if not max_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ], style={'margin-bottom': '30px'})
                ])
                tables.append(max_tables_row)
            
            # Average Latency Tables (side by side)
            if not avg_oneway_pivot.empty or not avg_roundtrip_pivot.empty:
                avg_tables_row = html.Div([
                    html.H4("Average Latency (μs)", style={'color': '#059669', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way average latency table
                        html.Div([
                            html.H5("One-way", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='avg-oneway-latency-table',
                                data=avg_oneway_pivot.to_dict('records') if not avg_oneway_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in avg_oneway_pivot.columns] if not avg_oneway_pivot.empty else [],
                                **table_style
                            ) if not avg_oneway_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip average latency table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='avg-roundtrip-latency-table',
                                data=avg_roundtrip_pivot.to_dict('records') if not avg_roundtrip_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in avg_roundtrip_pivot.columns] if not avg_roundtrip_pivot.empty else [],
                                **table_style
                            ) if not avg_roundtrip_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ])
                ])
                tables.append(avg_tables_row)
            
            latency_table_div = html.Div([
                html.H3("Latency Performance Data"),
                html.P(f"Latency measurements organized by message size and mechanism. Lower values indicate better performance.",
                       style={'color': '#6b7280', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                *tables
            ], className="dash-table-container")
        
        if throughput_data:
            # Create 2 separate throughput tables: One-way and Round-trip
            throughput_df = pd.DataFrame(throughput_data)
            
            # Filter data for each table type
            oneway_throughput_data = throughput_df[throughput_df['type'] == 'One-way']
            roundtrip_throughput_data = throughput_df[throughput_df['type'] == 'Round-trip']
            
            # Create pivot tables for messages/sec
            oneway_msgs_pivot = self.create_pivot_table(oneway_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=3)
            roundtrip_msgs_pivot = self.create_pivot_table(roundtrip_throughput_data.to_dict('records'), 'msgs_per_sec', round_digits=3)
            
            # Create pivot tables for bytes/sec (convert to MB/s for readability)
            oneway_throughput_mb = oneway_throughput_data.copy()
            roundtrip_throughput_mb = roundtrip_throughput_data.copy()
            if not oneway_throughput_mb.empty:
                oneway_throughput_mb['bytes_per_sec'] = oneway_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
            if not roundtrip_throughput_mb.empty:
                roundtrip_throughput_mb['bytes_per_sec'] = roundtrip_throughput_mb['bytes_per_sec'] / 1_000_000  # Convert to MB/s
                
            oneway_bytes_pivot = self.create_pivot_table(oneway_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
            roundtrip_bytes_pivot = self.create_pivot_table(roundtrip_throughput_mb.to_dict('records'), 'bytes_per_sec', round_digits=1)
            
            # Common table style for throughput tables
            throughput_table_style = {
                'style_table': {'overflowX': 'auto', 'width': '100%'},
                'style_cell': {'textAlign': 'center', 'padding': '8px', 'fontFamily': 'Arial, sans-serif', 'fontSize': '12px'},
                'style_header': {'backgroundColor': '#e0f2fe', 'fontWeight': 'bold', 'color': '#0277bd'},
                'style_data_conditional': [{'if': {'row_index': 'odd'}, 'backgroundColor': '#f8f9fa'}],
                'sort_action': "native"
            }
            
            throughput_tables = []
            
            # Messages/sec Tables (side by side)
            if not oneway_msgs_pivot.empty or not roundtrip_msgs_pivot.empty:
                msgs_tables_row = html.Div([
                    html.H4("Message Throughput (messages/sec)", style={'color': '#0277bd', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way messages/sec table
                        html.Div([
                            html.H5("One-way", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='oneway-msgs-throughput-table',
                                data=oneway_msgs_pivot.to_dict('records') if not oneway_msgs_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in oneway_msgs_pivot.columns] if not oneway_msgs_pivot.empty else [],
                                **throughput_table_style
                            ) if not oneway_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip messages/sec table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='roundtrip-msgs-throughput-table',
                                data=roundtrip_msgs_pivot.to_dict('records') if not roundtrip_msgs_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in roundtrip_msgs_pivot.columns] if not roundtrip_msgs_pivot.empty else [],
                                **throughput_table_style
                            ) if not roundtrip_msgs_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ], style={'margin-bottom': '30px'})
                ])
                throughput_tables.append(msgs_tables_row)
            
            # Bytes/sec Tables (side by side)
            if not oneway_bytes_pivot.empty or not roundtrip_bytes_pivot.empty:
                bytes_tables_row = html.Div([
                    html.H4("Data Throughput (MB/sec)", style={'color': '#0277bd', 'margin-bottom': '15px', 'text-align': 'center'}),
                    html.Div([
                        # One-way bytes/sec table
                        html.Div([
                            html.H5("One-way", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='oneway-bytes-throughput-table',
                                data=oneway_bytes_pivot.to_dict('records') if not oneway_bytes_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in oneway_bytes_pivot.columns] if not oneway_bytes_pivot.empty else [],
                                **throughput_table_style
                            ) if not oneway_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block', 'margin-right': '4%'}),
                        
                        # Round-trip bytes/sec table
                        html.Div([
                            html.H5("Round-trip", style={'color': '#374151', 'margin-bottom': '10px', 'text-align': 'center'}),
                            dash_table.DataTable(
                                id='roundtrip-bytes-throughput-table',
                                data=roundtrip_bytes_pivot.to_dict('records') if not roundtrip_bytes_pivot.empty else [],
                                columns=[{"name": i, "id": i} for i in roundtrip_bytes_pivot.columns] if not roundtrip_bytes_pivot.empty else [],
                                **throughput_table_style
                            ) if not roundtrip_bytes_pivot.empty else html.Div("No data available", style={'text-align': 'center', 'color': '#6b7280'})
                        ], style={'width': '48%', 'display': 'inline-block'})
                    ])
                ])
                throughput_tables.append(bytes_tables_row)
            
            throughput_table_div = html.Div([
                html.H3("Throughput Performance Data"),
                html.P(f"Throughput measurements organized by message size and mechanism. Higher values indicate better performance.",
                       style={'color': '#6b7280', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                *throughput_tables
            ], className="dash-table-container")

        # Create performance comparison matrix tables
        comparison_matrix_div = html.Div()
        p50_matrices_exist = not p50_comparison_matrix.empty if isinstance(p50_comparison_matrix, pd.DataFrame) else False
        max_matrices_exist = not max_comparison_matrix.empty if isinstance(max_comparison_matrix, pd.DataFrame) else False
        
        if p50_matrices_exist or max_matrices_exist:
            matrix_tables = []
            
            # P50 Latency Comparison Matrix
            if p50_matrices_exist:
                # Reset index to make mechanism names a column
                p50_display = p50_comparison_matrix.reset_index()
                p50_display.columns.name = None  # Remove column name
                
                # Format the matrix for better display
                p50_display = p50_display.round(1)
                for col in p50_display.columns:
                    if col != 'mechanism' and col != 'Overall Score':
                        p50_display[col] = p50_display[col].apply(lambda x: f"+{x}%" if x != '-' and x > 0 else f"{x}%" if x != '-' else x)
                
                p50_table = dash_table.DataTable(
                    id='p50-comparison-table',
                    data=p50_display.to_dict('records'),
                    columns=[{"name": str(i), "id": str(i)} for i in p50_display.columns],
                    style_table={'overflowX': 'auto'},
                    style_cell={'textAlign': 'center', 'padding': '12px', 'fontFamily': 'Arial, sans-serif', 'fontSize': '14px'},
                    style_header={'backgroundColor': '#e0f2fe', 'fontWeight': 'bold', 'color': '#0277bd'},
                    style_data_conditional=[
                        {'if': {'row_index': 'odd'}, 'backgroundColor': '#f8f9fa'},
                        {'if': {'column_id': 'mechanism'}, 'textAlign': 'left', 'fontWeight': 'bold'},
                        {'if': {'column_id': 'Overall Score'}, 'backgroundColor': '#fff3cd', 'fontWeight': 'bold'},
                    ],
                    sort_action="native",
                )
                
                matrix_tables.append(html.Div([
                    html.H4("P50 Latency Comparison", style={'color': '#1565c0', 'margin-bottom': '10px'}),
                    html.P("Relative P50 latency performance. Lower percentages = better typical performance.",
                           style={'color': '#6b7280', 'margin-bottom': '15px', 'fontSize': '0.9rem'}),
                    p50_table
                ], style={'margin-bottom': '30px'}))
            
            # Max Latency Comparison Matrix
            if max_matrices_exist:
                # Reset index to make mechanism names a column
                max_display = max_comparison_matrix.reset_index()
                max_display.columns.name = None  # Remove column name
                
                # Format the matrix for better display
                max_display = max_display.round(1)
                for col in max_display.columns:
                    if col != 'mechanism' and col != 'Overall Score':
                        max_display[col] = max_display[col].apply(lambda x: f"+{x}%" if x != '-' and x > 0 else f"{x}%" if x != '-' else x)
                
                max_table = dash_table.DataTable(
                    id='max-comparison-table',
                    data=max_display.to_dict('records'),
                    columns=[{"name": str(i), "id": str(i)} for i in max_display.columns],
                    style_table={'overflowX': 'auto'},
                    style_cell={'textAlign': 'center', 'padding': '12px', 'fontFamily': 'Arial, sans-serif', 'fontSize': '14px'},
                    style_header={'backgroundColor': '#fee2e2', 'fontWeight': 'bold', 'color': '#b91c1c'},
                    style_data_conditional=[
                        {'if': {'row_index': 'odd'}, 'backgroundColor': '#f8f9fa'},
                        {'if': {'column_id': 'mechanism'}, 'textAlign': 'left', 'fontWeight': 'bold'},
                        {'if': {'column_id': 'Overall Score'}, 'backgroundColor': '#fed7d7', 'fontWeight': 'bold'},
                    ],
                    sort_action="native",
                )
                
                matrix_tables.append(html.Div([
                    html.H4("Max Latency Comparison", style={'color': '#b91c1c', 'margin-bottom': '10px'}),
                    html.P("Relative maximum latency performance. Lower percentages = better worst-case performance.",
                           style={'color': '#6b7280', 'margin-bottom': '15px', 'fontSize': '0.9rem'}),
                    max_table
                ], style={'margin-bottom': '20px'}))
            
            comparison_matrix_div = html.Div([
                html.H3("Performance Comparison Matrix", style={'color': '#1565c0', 'margin-bottom': '15px'}),
                html.P("Comprehensive performance comparison across latency metrics. Lower percentages indicate better performance.",
                       style={'color': '#6b7280', 'margin-bottom': '25px', 'fontSize': '0.95rem'}),
                html.Div(matrix_tables)
            ], className="dash-table-container")

        # Organize layout into logical sections
        return html.Div([
            # === OVERVIEW SECTION ===
            dcc.Loading(
                id="performance-overview-loading",
                type="circle",
                children=html.Div([
                    html.H2("Performance Overview", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                    summary_cards
                ], style={'margin-bottom': '40px'}),
                style={'minHeight': '150px'}
            ),
            
            # === PERFORMANCE COMPARISON SECTION ===
            dcc.Loading(
                id="comparison-matrix-loading",
                type="cube",
                children=html.Div([
                    html.H2("Head-to-Head Comparison", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                    comparison_matrix_div
                ], style={'margin-bottom': '40px'}),
                style={'minHeight': '200px'}
            ),
            
            # === INSIGHTS SECTION ===
            html.Div([
                html.H2("Performance Insights", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                html.Div([
                    html.Div([
                        html.H4("Recommendations:", style={'color': '#059669', 'margin-bottom': '15px'}),
                        html.Ul([
                            html.Li(rec, style={'margin-bottom': '8px', 'color': '#374151'}) 
                            for rec in insights.get('recommendations', ['Select data to see recommendations'])
                        ], style={'padding-left': '20px'})
                    ], style={
                        'background': 'linear-gradient(135deg, #f0fdf4 0%, #dcfce7 100%)',
                        'padding': '25px', 'border-radius': '12px',
                        'box-shadow': '0 4px 6px rgba(0, 0, 0, 0.1)', 'border': '1px solid #10b981'
                    })
                ], className="chart-container")
            ], style={'margin-bottom': '40px'}),
            
            # === LATENCY ANALYSIS SECTION ===
            dcc.Loading(
                id="latency-analysis-loading",
                type="default",
                children=html.Div([
                    html.H2("Latency Analysis", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
                html.Div([
                    dcc.Graph(id='avg-latency-chart', figure=avg_latency_fig)
            ], className="chart-container"),
                html.Div([
                    dcc.Graph(id='max-latency-chart', figure=max_latency_fig)
                ], className="chart-container"),
                    latency_table_div if latency_data else html.Div()
                ], style={'margin-bottom': '40px'}),
                style={'minHeight': '300px'}
            ),
            
            # === THROUGHPUT ANALYSIS SECTION ===
            dcc.Loading(
                id="throughput-analysis-loading",
                type="default",
                children=html.Div([
                    html.H2("Throughput Analysis", style={'color': '#1f2937', 'margin-bottom': '20px', 'font-size': '1.5rem'}),
            html.Div([
                dcc.Graph(id='throughput-msgs-chart', figure=msgs_fig)
            ], className="chart-container"),
            html.Div([
                dcc.Graph(id='throughput-bytes-chart', figure=bytes_fig)
            ], className="chart-container"),
                    throughput_table_div if throughput_data else html.Div()
                ], style={'margin-bottom': '40px'}),
                style={'minHeight': '300px'}
            )
        ])
    
    def render_timeseries_tab(self, mechanisms: List, message_sizes: List):
        """Render time series analysis tab."""
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        if streaming_df.empty:
            return html.Div([
                html.H3("No Data Available"),
                html.P("No streaming data available for the selected filters. Try adjusting your filter selections.", 
                       style={'color': '#6b7280', 'fontSize': '1.1rem', 'textAlign': 'center', 'padding': '40px'})
            ], style={'textAlign': 'center', 'padding': '50px'})
            
        # Create time series controls section
        available_mechanisms = mechanisms if mechanisms and len(mechanisms) > 0 else streaming_df['mechanism'].unique().tolist()
        
        controls_section = html.Div([
            html.H3("Time Series Controls", style={'margin-bottom': '20px'}),
            
            # Quick Preset Configuration Buttons
            html.Div([
                html.Label("Quick Presets:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                html.Div([
                    html.Button("Performance Analysis", id="preset-performance", n_clicks=0, 
                              className="preset-btn", style={'margin-right': '10px', 'margin-bottom': '8px'},
                              title="Separate charts with performance zones and percentile bands for overall performance analysis"),
                    html.Button("Detailed Inspection", id="preset-detailed", n_clicks=0, 
                              className="preset-btn", style={'margin-right': '10px', 'margin-bottom': '8px'},
                              title="Separate charts with spike detection and distribution overlays for detailed mechanism analysis"),
                    html.Button("Statistical Overview", id="preset-statistical", n_clicks=0, 
                              className="preset-btn", style={'margin-right': '10px', 'margin-bottom': '8px'},
                              title="Faceted view by message size with rolling percentiles and statistical overlays"),
                    html.Button("Outlier Detection", id="preset-outliers", n_clicks=0, 
                              className="preset-btn", style={'margin-right': '10px', 'margin-bottom': '8px'},
                              title="Separate charts with spike and anomaly detection for identifying outliers"),
                    html.Button("Reset to Default", id="preset-reset", n_clicks=0, 
                              className="preset-btn-secondary", style={'margin-bottom': '8px'},
                              title="Reset all controls to default settings (preserves statistics panel state)"),
                ]),
            ], style={'margin-bottom': '20px', 'padding': '15px', 'background': '#fef7cd', 'border-radius': '8px', 'border': '1px solid #f59e0b'}),
            
            # Chart Layout & Statistical Controls
            html.Div([
                html.Label("Chart Layout & Statistical Overlays:", style={'margin-bottom': '15px', 'display': 'block', 'font-weight': 'bold', 'font-size': '1.1em'}),
                html.Div([
                    # Chart Layout
                    html.Div([
                        html.Label("Chart Layout:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                        html.Div([
                            dcc.RadioItems(
                                id='ts-chart-layout',
                                options=[
                                    {'label': 'Separate Charts (by mechanism)', 'value': 'separate'},
                                    {'label': 'Separate Charts (by message size)', 'value': 'faceted'}
                                ],
                                value='separate',
                                className="chart-layout-radio",
                                style={'margin-bottom': '15px'}
                            ),
                        ], title="Choose how to organize charts: separate charts for each mechanism, or separate charts for each message size"),
                        
                        # Statistical Overlays in same column
                        html.Label("Statistical Overlays:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                        html.Div([
                            dcc.Checklist(
                                id='ts-statistical-overlays',
                                options=[
                                    {'label': 'Percentile Bands (P25-P75)', 'value': 'percentile_bands'},
                                    {'label': 'Spike Detection (Z-score outliers)', 'value': 'spike_detection'},
                                    {'label': 'Anomaly Detection (ML pattern analysis)', 'value': 'anomaly_detection'},
                                    {'label': 'Distribution Overlays', 'value': 'distributions'},
                                    {'label': 'Rolling P95/P99', 'value': 'rolling_percentiles'}
                                ],
                                value=['percentile_bands'],
                                className="filter-checklist"
                            ),
                        ], title="Statistical overlays: confidence bands, spike/anomaly detection, mini histogram overlays, rolling percentile calculations"),
                    ], style={'width': '48%', 'display': 'inline-block', 'vertical-align': 'top'}),
                    
                    # View Options and Latency Type
                    html.Div([
                        html.Label("View Options:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                        html.Div([
                            dcc.Checklist(
                                id='ts-view-options',
                                options=[
                                    {'label': 'Sync Zoom/Pan', 'value': 'sync_zoom'},
                                    {'label': 'Show Statistics Panel', 'value': 'show_stats'},
                                    {'label': 'Enable Crossfilter', 'value': 'crossfilter'},
                                    {'label': 'Performance Zones', 'value': 'perf_zones'}
                                ],
                                value=['sync_zoom'],
                                className="filter-checklist"
                            ),
                        ], title="View options: sync zoom/pan across charts, show detailed statistics, enable interactive filtering, highlight performance zones"),
                        
                        html.Br(),
                        html.Label("Latency Type:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                        html.Div([
                            dcc.Checklist(
                                id='ts-latency-types',
                                options=[
                                    {'label': 'One-way Latency', 'value': 'one_way'},
                                    {'label': 'Round-trip Latency', 'value': 'round_trip'}
                                ],
                                value=['one_way'],
                                className="filter-checklist"
                            ),
                        ], title="Select which latency types to display in charts and statistics: one-way (send only) or round-trip (send + receive)"),
                    ], style={'width': '48%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '4%'}),
                ])
            ], style={'margin-bottom': '30px', 'padding': '15px', 'background': '#f8fafc', 'border-radius': '8px', 'border': '1px solid #e2e8f0'}),
            
            html.Div([
                html.Div([
                    html.Label("Moving Average Window:", style={'margin-bottom': '10px', 'display': 'block'}),
                    dcc.Slider(
                        id='ts-moving-avg-slider',
                        min=1,
                        max=1000,
                        step=1,
                        value=100,
                        marks={i: str(i) for i in [1, 100, 500, 1000]},
                        tooltip={"placement": "bottom", "always_visible": True}
                    ),
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top'}),
                
                html.Div([
                    html.Label("Display Options:", style={'margin-bottom': '10px', 'display': 'block'}),
                    dcc.Checklist(
                        id='ts-display-options',
                        options=[
                            {'label': 'Show Raw Data Points', 'value': 'raw_dots'},
                            {'label': 'Show Moving Average', 'value': 'moving_avg'}
                        ],
                        value=['raw_dots', 'moving_avg'],
                        className="filter-checklist"
                    ),
                ], style={'width': '25%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '3%'}),
                
                html.Div([
                    html.Label("Y-Axis Options:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                    html.Div([
                        dcc.RadioItems(
                            id='ts-y-axis-scale',
                            options=[
                                {'label': 'Linear Scale', 'value': 'linear'},
                                {'label': 'Log Scale', 'value': 'log'}
                            ],
                            value='log',
                            className="axis-scale-radio"
                        ),
                    ], title="Y-axis scaling: linear for normal data, logarithmic for wide value ranges or highlighting multiplicative differences"),
                    html.Br(),
                    html.Label("Range:", style={'margin-bottom': '5px', 'display': 'block', 'font-size': '0.9rem'}),
                    dcc.RadioItems(
                        id='ts-y-axis-range',
                        options=[
                            {'label': 'Auto', 'value': 'auto'},
                            {'label': 'Fixed', 'value': 'fixed'}
                        ],
                        value='auto',
                        className="axis-range-radio"
                    ),
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '3%'}),
            
            ], style={'margin-bottom': '30px'}),
            
            # Sampling Controls
            html.Div([
                
                html.Div([
                    html.Label("Sampling Strategy:", style={'margin-bottom': '10px', 'display': 'block', 'font-weight': 'bold'}),
                    html.Div([
                        dcc.Dropdown(
                            id='ts-sampling-strategy',
                            options=[
                                {'label': 'Uniform Sampling', 'value': 'uniform'},
                                {'label': 'Peak-Preserving', 'value': 'peak_preserving'},
                                {'label': 'Outlier-Preserving', 'value': 'outlier_preserving'},
                                {'label': 'Adaptive Sampling', 'value': 'adaptive'}
                            ],
                            value='uniform',
                            className="sampling-dropdown"
                        ),
                    ], title="Data sampling for large datasets: uniform (even spacing), peak-preserving (maintains peaks), outlier-preserving (keeps extreme values), adaptive (smart density-based)"),
                ], style={'width': '30%', 'display': 'inline-block', 'vertical-align': 'top', 'margin-left': '3%'}),
                

            ], style={'margin-bottom': '30px'}),
            

            
            # Charts container that will be updated
            dcc.Loading(
                id="ts-charts-loading",
                type="default",
                children=html.Div(id='ts-charts-container'),
                style={'minHeight': '400px'}
            ),
            
            # Real-time Statistics Panel (conditional)
            dcc.Loading(
                id="ts-stats-loading",
                type="dot",
                children=html.Div(id='ts-stats-panel', style={'margin-bottom': '20px'}),
                style={'minHeight': '50px'}
            )
        ], className="filter-section", style={'margin-bottom': '30px'})
        
        return controls_section
    
    def render_timeseries_charts(self, mechanisms: List, message_sizes: List, 
                                moving_avg_window: int, display_options: List,
                                chart_layout: str = 'separate',
                                view_options: List = None, latency_types: List = None, statistical_overlays: List = None, 
                                y_axis_scale: str = 'log', 
                                y_axis_range: str = 'auto', sampling_strategy: str = 'uniform'):
        """Render the actual time series charts with advanced features and multiple layout options."""
        # Initialize default parameters
        view_options = view_options or ['sync_zoom']
        latency_types = latency_types or ['one_way']
        statistical_overlays = statistical_overlays or ['percentile_bands']
        
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        if streaming_df.empty:
            return html.Div([
                html.P("No data available for current selections.", 
                       style={'textAlign': 'center', 'color': '#6b7280', 'fontSize': '1.1rem', 'padding': '40px'})
            ])

        # Advanced sampling strategies
        MAX_POINTS_PER_RUN = 10000
        total_original_points = len(streaming_df)
        performance_note = html.Div()
        
        def apply_sampling_strategy(df, strategy, max_points):
            """Apply different sampling strategies to the dataframe."""
            if len(df) <= max_points:
                return df
                
            if strategy == 'uniform':
                # Standard uniform sampling
                step_size = len(df) // max_points
                return df.iloc[::step_size].copy()
            
            elif strategy == 'peak_preserving':
                # Preserve peaks and valleys in the data
                uniform_sample = df.iloc[::len(df)//max_points].copy()
                # Add peaks (highest values)
                peaks = df.nlargest(max_points//4, 'one_way_latency_us')
                # Add valleys (lowest values)  
                valleys = df.nsmallest(max_points//4, 'one_way_latency_us')
                combined = pd.concat([uniform_sample, peaks, valleys]).drop_duplicates().sort_index()
                return combined.iloc[:max_points] if len(combined) > max_points else combined
            
            elif strategy == 'outlier_preserving':
                # Focus on preserving outliers and anomalies
                q75 = df['one_way_latency_us'].quantile(0.75)
                q25 = df['one_way_latency_us'].quantile(0.25)
                iqr = q75 - q25
                outlier_threshold = q75 + 1.5 * iqr
                
                # Get all outliers
                outliers = df[df['one_way_latency_us'] > outlier_threshold]
                # Get uniform sample of non-outliers
                non_outliers = df[df['one_way_latency_us'] <= outlier_threshold]
                if len(non_outliers) > 0:
                    step_size = max(1, len(non_outliers) // (max_points - len(outliers)))
                    sampled_normal = non_outliers.iloc[::step_size]
                else:
                    sampled_normal = pd.DataFrame()
                
                combined = pd.concat([sampled_normal, outliers]).drop_duplicates().sort_index()
                return combined.iloc[:max_points] if len(combined) > max_points else combined
            
            elif strategy == 'adaptive':
                # Adaptive sampling based on variance
                # More samples in high-variance regions
                df_copy = df.copy().reset_index(drop=True)
                df_copy['variance_window'] = df_copy['one_way_latency_us'].rolling(window=100, min_periods=1).var()
                
                # Sample more from high-variance regions
                high_variance = df_copy[df_copy['variance_window'] > df_copy['variance_window'].median()]
                low_variance = df_copy[df_copy['variance_window'] <= df_copy['variance_window'].median()]
                
                high_var_samples = min(len(high_variance), max_points * 2 // 3)
                low_var_samples = max_points - high_var_samples
                
                if len(high_variance) > 0 and high_var_samples > 0:
                    high_step = max(1, len(high_variance) // high_var_samples)
                    sampled_high = high_variance.iloc[::high_step]
                else:
                    sampled_high = pd.DataFrame()
                
                if len(low_variance) > 0 and low_var_samples > 0:
                    low_step = max(1, len(low_variance) // low_var_samples)
                    sampled_low = low_variance.iloc[::low_step]
                else:
                    sampled_low = pd.DataFrame()
                
                combined = pd.concat([sampled_high, sampled_low]).drop_duplicates().sort_index()
                return combined.iloc[:max_points] if len(combined) > max_points else combined
            
            else:
                # Fallback to uniform
                step_size = len(df) // max_points
                return df.iloc[::step_size].copy()
        
        if total_original_points > MAX_POINTS_PER_RUN:
            strategy_info = {
                'uniform': {
                    'name': 'Uniform Sampling',
                    'definition': 'Takes every Nth data point at regular intervals, like picking every 10th sample.',
                    'pros': 'Fast and consistent; gives balanced view across time',
                    'cons': 'May miss important spikes or rare events between samples'
                },
                'peak_preserving': {
                    'name': 'Peak-Preserving Sampling', 
                    'definition': 'Combines regular sampling with the highest and lowest values to keep important extremes.',
                    'pros': 'Captures performance spikes and dips; good for identifying bottlenecks',
                    'cons': 'Slightly slower; may over-emphasize outliers in analysis'
                },
                'outlier_preserving': {
                    'name': 'Outlier-Preserving Sampling',
                    'definition': 'Focuses on keeping unusual values (outliers) while sampling normally elsewhere.',
                    'pros': 'Excellent for anomaly detection and debugging unusual behavior',
                    'cons': 'May skew statistical analysis; less representative of typical performance'
                },
                'adaptive': {
                    'name': 'Adaptive Variance-Based Sampling',
                    'definition': 'Smart sampling that keeps more data points where values change rapidly.',
                    'pros': 'Best balance of performance and detail; adapts to data complexity',
                    'cons': 'Most computationally expensive; may be overkill for simple datasets'
                }
            }
            
            current_strategy = strategy_info.get(sampling_strategy, strategy_info['uniform'])
            
            performance_note = html.Div([
                html.P(f"Performance Note: Using {current_strategy['name']} for better interactivity. "
                       f"Original: {total_original_points:,} points, "
                       f"Displaying: ~{MAX_POINTS_PER_RUN:,} points.", 
                       style={'margin-bottom': '8px'}),
                html.P([
                    html.Strong("What it does: "), 
                    current_strategy['definition']
                ], style={'font-size': '0.85rem', 'margin-bottom': '5px'}),
                html.P([
                    html.Strong("Pros: ", style={'color': 'black'}), 
                    current_strategy['pros']
                ], style={'font-size': '0.85rem', 'margin-bottom': '3px'}),
                html.P([
                    html.Strong("Cons: ", style={'color': 'black'}), 
                    current_strategy['cons']
                ], style={'font-size': '0.85rem', 'color': 'black'})
            ], className="performance-note")

        # Advanced Chart Generation with Multiple Layout Options
        # Mechanism-specific color scheme with latency type variations
        def get_mechanism_colors(mechanism, latency_type):
            """Generate mechanism-specific colors with latency type variations."""
            # Base colors for mechanisms (more distinct colors)
            base_colors = {
                'SharedMemory': '#3b82f6',      # Blue family
                'TcpSocket': '#10b981',         # Green family  
                'UnixDomainSocket': '#8b5cf6',  # Purple family
                'PosixMessageQueue': '#f59e0b', # Orange family
                'NamedPipe': '#ef4444',         # Red family
                'FileIO': '#06b6d4',            # Cyan family
            }
            
            # Get base color or use a default if mechanism not in mapping
            available_mechanisms = list(streaming_df['mechanism'].unique()) if len(streaming_df) > 0 else []
            if mechanism not in base_colors:
                # Assign colors cyclically for unknown mechanisms
                fallback_colors = ['#6b7280', '#84cc16', '#e11d48', '#14b8a6', '#f97316']
                idx = available_mechanisms.index(mechanism) % len(fallback_colors) if mechanism in available_mechanisms else 0
                base_color = fallback_colors[idx]
            else:
                base_color = base_colors[mechanism]
            
            # Create variations for latency types
            if latency_type == 'one_way':
                return base_color  # Use base color for one-way
            else:  # round_trip
                # Create a darker/more saturated version for round-trip
                # Convert hex to RGB, darken it, convert back
                hex_color = base_color.lstrip('#')
                rgb = tuple(int(hex_color[i:i+2], 16) for i in (0, 2, 4))
                # Darken by reducing values by 20%
                darkened_rgb = tuple(max(0, int(c * 0.75)) for c in rgb)
                return f"#{darkened_rgb[0]:02x}{darkened_rgb[1]:02x}{darkened_rgb[2]:02x}"
        
        # Symbol scheme: different symbols for each mechanism
        symbols = ['circle', 'square', 'diamond', 'cross', 'x', 'triangle-up', 'triangle-down', 'star']
        mechanism_colors = ['#3b82f6', '#ef4444', '#10b981', '#f59e0b', '#8b5cf6', '#f97316', '#06b6d4', '#84cc16']
        
        charts = []
        stats_panel = html.Div()
        
        # Generate statistics panel if requested
        if 'show_stats' in view_options:
            stats_data = []
            for mechanism in streaming_df['mechanism'].unique():
                mech_data = streaming_df[streaming_df['mechanism'] == mechanism]
                if len(mech_data) > 0:
                    stats_data.append({
                        'Mechanism': mechanism,
                        'Count': f"{len(mech_data):,}",
                        'Mean (μs)': f"{mech_data['one_way_latency_us'].mean():.2f}",
                        'Std (μs)': f"{mech_data['one_way_latency_us'].std():.2f}",
                        'Min (μs)': f"{mech_data['one_way_latency_us'].min():.2f}",
                        'Max (μs)': f"{mech_data['one_way_latency_us'].max():.2f}",
                        'P95 (μs)': f"{mech_data['one_way_latency_us'].quantile(0.95):.2f}",
                        'P99 (μs)': f"{mech_data['one_way_latency_us'].quantile(0.99):.2f}"
                    })
            
            if stats_data:
                stats_panel = html.Div([
                    html.H4("Real-time Statistics", style={'margin-bottom': '15px', 'color': '#1f2937'}),
                    dash_table.DataTable(
                        data=stats_data,
                        columns=[{"name": col, "id": col} for col in stats_data[0].keys()],
                        style_cell={'textAlign': 'left', 'padding': '10px', 'font-size': '0.85rem'},
                        style_header={'backgroundColor': '#f8fafc', 'fontWeight': 'bold', 'border': '1px solid #e2e8f0'},
                        style_data={'backgroundColor': '#ffffff', 'border': '1px solid #e2e8f0'},
                        style_table={'border-radius': '8px', 'overflow': 'hidden'},
                    )
                ], style={'margin-bottom': '30px', 'padding': '20px', 'background': '#f8fafc', 'border-radius': '8px', 'border': '1px solid #e2e8f0'})
        
        # Helper function to add statistical overlays
        def add_statistical_overlays(fig, data, mechanism, color, latency_type='one_way'):
            """Add percentile bands and other statistical overlays."""
            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
            
            # Skip if column doesn't exist in data
            if latency_col not in data.columns or data[latency_col].isna().all():
                return
                
            # Add visual overlays for spikes and anomalies
            if ('spike_detection' in statistical_overlays or 'anomaly_detection' in statistical_overlays) and len(data) > 20:
                anomalies = detect_anomalies(data, mechanism, latency_type)
                
                for anomaly in anomalies:
                    anomaly_data = anomaly['data']
                    if len(anomaly_data) > 0:
                        if anomaly['type'] == 'spikes':
                            # Add spike markers (using magenta to avoid conflicts with red mechanism colors)
                            fig.add_trace(go.Scatter(
                                x=anomaly_data.index,
                                y=anomaly_data[latency_col],
                                mode='markers',
                                name=f'Spikes ({mechanism} {type_label})',
                                marker=dict(
                                    color='#ec4899',  # Magenta - distinct from all mechanism colors
                                    size=8,
                                    symbol='triangle-up',
                                    line=dict(width=2, color='#be185d')  # Dark magenta border
                                ),
                                showlegend=True,
                                hovertemplate=f'<b>Spike Detected</b><br>%{{x}}<br>%{{y:.2f}} μs<extra></extra>'
                            ))
                        else:  # anomalies or statistical_anomalies
                            # Add anomaly markers (using brown to avoid conflicts with all mechanism colors)
                            fig.add_trace(go.Scatter(
                                x=anomaly_data.index,
                                y=anomaly_data[latency_col],
                                mode='markers',
                                name=f'Anomalies ({mechanism} {type_label})',
                                marker=dict(
                                    color='#a16207',  # Brown - distinct from all mechanism colors
                                    size=6,
                                    symbol='diamond',
                                    line=dict(width=1, color='#713f12')  # Dark brown border
                                ),
                                showlegend=True,
                                hovertemplate=f'<b>Anomaly Detected</b><br>%{{x}}<br>%{{y:.2f}} μs<extra></extra>'
                            ))
                
            if 'percentile_bands' in statistical_overlays and len(data) > 50:
                # Add P25-P75 confidence bands
                data_sorted = data.sort_index()
                window_size = min(100, len(data) // 10)
                
                if window_size > 1:
                    p25_rolling = data_sorted[latency_col].rolling(window=window_size, min_periods=1).quantile(0.25)
                    p75_rolling = data_sorted[latency_col].rolling(window=window_size, min_periods=1).quantile(0.75)
                    
                    # Add percentile band
                    fig.add_trace(go.Scatter(
                        x=data_sorted.index,
                        y=p75_rolling,
                        mode='lines',
                        name=f'P75 ({mechanism} {type_label})',
                        line=dict(color=color, width=1, dash='dot'),
                        opacity=0.4,
                        showlegend=False,
                        hoverinfo='skip'
                    ))
                    
                    # Convert color to rgba for fill
                    try:
                        import matplotlib.colors as mcolors
                        rgb = mcolors.to_rgb(color)
                        fillcolor = f'rgba({int(rgb[0]*255)}, {int(rgb[1]*255)}, {int(rgb[2]*255)}, 0.15)'
                    except:
                        fillcolor = 'rgba(59, 130, 246, 0.15)'  # fallback blue
                    
                    fig.add_trace(go.Scatter(
                        x=data_sorted.index,
                        y=p25_rolling,
                        mode='lines',
                        name=f'P25-P75 Band ({mechanism} {type_label})',
                        line=dict(color=color, width=1, dash='dot'),
                        fill='tonexty',
                        fillcolor=fillcolor,
                        opacity=0.4,
                        showlegend=True,
                        hoverinfo='skip',
                        hovertemplate=f'<b>Percentile Band</b><br>P25: %{{y:.2f}} μs<br>Index: %{{x}}<extra></extra>'
                    ))
            
            if 'rolling_percentiles' in statistical_overlays and len(data) > 100:
                # Add rolling P95 and P99 percentiles
                data_sorted = data.sort_index()
                window_size = min(200, len(data) // 5)
                
                if window_size > 10:
                    p95_rolling = data_sorted[latency_col].rolling(window=window_size, min_periods=10).quantile(0.95)
                    p99_rolling = data_sorted[latency_col].rolling(window=window_size, min_periods=10).quantile(0.99)
                    
                    # Add P95 line
                    fig.add_trace(go.Scatter(
                        x=data_sorted.index,
                        y=p95_rolling,
                        mode='lines',
                        name=f'P95 Rolling ({mechanism} {type_label})',
                        line=dict(color=color, width=2, dash='dashdot'),
                        opacity=0.8,
                        showlegend=True,
                        hovertemplate=f'<b>P95 Rolling</b><br>Value: %{{y:.2f}} μs<br>Index: %{{x}}<extra></extra>'
                    ))
                    
                    # Add P99 line
                    fig.add_trace(go.Scatter(
                        x=data_sorted.index,
                        y=p99_rolling,
                        mode='lines',
                        name=f'P99 Rolling ({mechanism} {type_label})',
                        line=dict(color=color, width=2, dash='longdash'),
                        opacity=0.8,
                        showlegend=True,
                        hovertemplate=f'<b>P99 Rolling</b><br>Value: %{{y:.2f}} μs<br>Index: %{{x}}<extra></extra>'
                    ))
            
            if 'distributions' in statistical_overlays and len(data) > 50:
                # Add distribution overlay as a small histogram in upper right corner
                latency_values = data[latency_col].dropna()
                
                if len(latency_values) > 20:
                    # Calculate histogram
                    hist_counts, hist_bins = np.histogram(latency_values, bins=20)
                    hist_centers = (hist_bins[:-1] + hist_bins[1:]) / 2
                    
                    # Normalize counts to fit in a small area (scale to 10% of y-range)
                    y_range = fig.layout.yaxis.range if fig.layout.yaxis.range else [latency_values.min(), latency_values.max()]
                    if not y_range or y_range[0] == y_range[1]:
                        y_range = [latency_values.min(), latency_values.max()]
                    
                    y_span = y_range[1] - y_range[0]
                    normalized_counts = (hist_counts / hist_counts.max()) * (y_span * 0.1) + y_range[1] * 0.85
                    
                    # Add histogram bars as scatter plot
                    fig.add_trace(go.Scatter(
                        x=normalized_counts,
                        y=hist_centers,
                            mode='lines',
                        name=f'Distribution ({mechanism} {type_label})',
                        line=dict(color=color, width=2),
                        opacity=0.6,
                        showlegend=True,
                        hovertemplate=f'<b>Distribution</b><br>Latency: %{{y:.2f}} μs<br>Frequency: %{{x:.0f}}<extra></extra>',
                        xaxis='x2',
                        yaxis='y'
                    ))

        
        # Helper function to detect spikes and anomalies
        def detect_anomalies(data, mechanism, latency_type='one_way'):
            """Detect spikes and anomalies in the data."""
            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
            anomalies = []
            
            # Skip if column doesn't exist in data
            if latency_col not in data.columns or data[latency_col].isna().all():
                return anomalies
            
            if 'spike_detection' in statistical_overlays and len(data) > 20:
                # Simple spike detection using z-score
                try:
                    from scipy.stats import zscore
                    z_scores = np.abs(zscore(data[latency_col]))
                    spike_threshold = 3.0
                    spikes = data[z_scores > spike_threshold]
                    
                    if len(spikes) > 0:
                        anomalies.append({
                            'type': 'spikes',
                            'data': spikes,
                            'description': f"{len(spikes)} latency spikes detected in {mechanism} ({type_label})"
                        })
                except ImportError:
                    pass  # Skip spike detection if scipy not available
            
            if 'anomaly_detection' in statistical_overlays and len(data) > 100:
                # Advanced anomaly detection using isolation forest
                if SKLEARN_AVAILABLE:
                    try:
                        clf = IsolationForest(contamination=0.1, random_state=42)
                        anomaly_labels = clf.fit_predict(data[[latency_col]].values)
                        anomaly_data = data[anomaly_labels == -1]
                        
                        if len(anomaly_data) > 0:
                            anomalies.append({
                                'type': 'anomalies',
                                'data': anomaly_data,
                                'description': f"{len(anomaly_data)} ML anomalies detected in {mechanism} ({type_label})"
                            })
                    except Exception:
                        # Fallback to simple statistical anomaly detection
                        q75 = data[latency_col].quantile(0.75)
                        q25 = data[latency_col].quantile(0.25)
                        iqr = q75 - q25
                        anomaly_threshold = q75 + 2.0 * iqr
                        anomaly_data = data[data[latency_col] > anomaly_threshold]
                        
                        if len(anomaly_data) > 0:
                            anomalies.append({
                                'type': 'statistical_anomalies',
                                'data': anomaly_data,
                                'description': f"{len(anomaly_data)} statistical anomalies in {mechanism} ({type_label})"
                            })

            
            return anomalies
        
        # Main chart generation logic based on layout type
        if chart_layout == 'faceted':
            # FACETED VIEW: Organize by message size
            message_sizes_available = sorted(streaming_df['message_size'].unique())
            
            for msg_size in message_sizes_available:
                size_data = streaming_df[streaming_df['message_size'] == msg_size]
                
                if len(size_data) > 0:
                    fig = go.Figure()
                    
                    for i, mechanism in enumerate(size_data['mechanism'].unique()):
                        symbol = symbols[i % len(symbols)]
                        color = mechanism_colors[i % len(mechanism_colors)]
                        
                        mechanism_df = size_data[size_data['mechanism'] == mechanism]
                        run_data = apply_sampling_strategy(mechanism_df, sampling_strategy, MAX_POINTS_PER_RUN)
                        run_data = run_data.reset_index(drop=True)
                        
                        # Add statistical overlays for each selected latency type
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            if latency_col in run_data.columns and not run_data[latency_col].isna().all():
                                overlay_color = get_mechanism_colors(mechanism, latency_type)
                                add_statistical_overlays(fig, run_data, mechanism, overlay_color, latency_type)
                        

                        
                        # Generate traces for each selected latency type
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                            trace_color = get_mechanism_colors(mechanism, latency_type)
                            
                            # Skip if column doesn't exist in data
                            if latency_col not in run_data.columns or run_data[latency_col].isna().all():
                                continue
                                
                            if 'raw_dots' in display_options:
                                fig.add_trace(go.Scatter(
                                    x=run_data.index,
                                    y=run_data[latency_col],
                                    mode='markers',
                                    name=f'{mechanism} ({type_label})',
                                    marker=dict(color=trace_color, size=3, opacity=0.7, symbol=symbol),
                            showlegend=True
                        ))
                        
                            if 'moving_avg' in display_options and len(run_data) > moving_avg_window:
                                ma_col = f'{latency_type}_ma'
                                run_data[ma_col] = run_data[latency_col].rolling(window=moving_avg_window, min_periods=1).mean()
                                
                            fig.add_trace(go.Scatter(
                                x=run_data.index,
                                    y=run_data[ma_col],
                                mode='lines',
                                    name=f'{mechanism} ({type_label}) MA',
                                    line=dict(color=trace_color, width=2, dash='solid'),
                                opacity=0.9,
                                showlegend=True
                            ))
            
                    # Add performance zones if enabled
                    if 'perf_zones' in view_options and len(size_data) > 0:
                        # Add performance zones for each selected latency type
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                            
                            if latency_col in size_data.columns and not size_data[latency_col].isna().all():
                                overall_p50 = size_data[latency_col].quantile(0.5)
                                overall_p95 = size_data[latency_col].quantile(0.95)
                                
                                # Use distinct colors for performance zones of different latency types
                                zone_color_p50 = '#10b981' if latency_type == 'one_way' else '#059669'  # Green family
                                zone_color_p95 = '#f59e0b' if latency_type == 'one_way' else '#d97706'  # Orange family
                                
                                fig.add_hline(y=overall_p50, line_dash="dash", line_color=zone_color_p50, 
                                             annotation_text=f"Good P50 ({type_label})", annotation_position="bottom right")
                                fig.add_hline(y=overall_p95, line_dash="dash", line_color=zone_color_p95, 
                                             annotation_text=f"Warning P95 ({type_label})", annotation_position="bottom right")
                    
                    # Update chart layout for this message size (moved inside chart creation block)
                    fig.update_layout(
                        title=f"Message Size: {msg_size} bytes",
                        xaxis_title="Sample Index", 
                        yaxis_title="Latency (μs)",
                        yaxis_type=y_axis_scale,
                        yaxis_autorange=True if y_axis_range == 'auto' else False,
                        showlegend=True,
                        font=dict(size=12),
                        height=500,
                        template='plotly_white',
                        legend=dict(
                            orientation="h",
                            yanchor="top",
                            y=-0.15,
                            xanchor="center",
                            x=0.5,
                            bgcolor="rgba(255,255,255,0.9)",
                            bordercolor="rgba(0,0,0,0.2)",
                            borderwidth=1
                        ),
                        margin=dict(b=100)
                    )
            
                    charts.append(html.Div([
                        dcc.Graph(figure=fig, id=f'faceted-chart-{msg_size}')
                    ], className="chart-container", style={'width': '100%', 'margin-bottom': '20px'}))
        
        else:
            # SEPARATE VIEW: Original behavior with enhancements
            for i, mechanism in enumerate(streaming_df['mechanism'].unique()):
                symbol = symbols[i % len(symbols)]
                color = mechanism_colors[i % len(mechanism_colors)]
                fig = go.Figure()
                
                # Filter data for this mechanism
                mechanism_df = streaming_df[streaming_df['mechanism'] == mechanism]
                
                if len(mechanism_df) > 0:
                    # Get unique message sizes for this mechanism to create separate traces
                    mechanism_message_sizes = sorted(mechanism_df['message_size'].unique())
                    
                    # Color palette for message sizes (distinct colors)
                    size_colors = ['#1f77b4', '#ff7f0e', '#2ca02c', '#d62728', '#9467bd', '#8c564b', '#e377c2', '#7f7f7f']
                    
                    anomalies = []
                    
                    # Create traces for each message size within this mechanism
                    for size_idx, msg_size in enumerate(mechanism_message_sizes):
                        # Filter data for this specific message size
                        size_df = mechanism_df[mechanism_df['message_size'] == msg_size]
                        
                        if len(size_df) == 0:
                            continue
                            
                        run_data = apply_sampling_strategy(size_df, sampling_strategy, MAX_POINTS_PER_RUN // len(mechanism_message_sizes))
                        run_data = run_data.reset_index(drop=True)
                        
                        # Use different colors for each message size
                        size_color = size_colors[size_idx % len(size_colors)]
                        
                        # Detect anomalies for each selected latency type for this message size
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            if latency_col in run_data.columns and not run_data[latency_col].isna().all():
                                type_anomalies = detect_anomalies(run_data, mechanism, latency_type)
                                anomalies.extend(type_anomalies)
                        
                        # Generate traces for each selected latency type and message size combination
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                            
                            # Skip if column doesn't exist in data
                            if latency_col not in run_data.columns or run_data[latency_col].isna().all():
                                continue
                            
                            # Create trace name with both message size and latency type
                            trace_name = f'{msg_size}B - {type_label}'
                            
                            if 'raw_dots' in display_options:
                                fig.add_trace(go.Scatter(
                                    x=run_data.index,
                                    y=run_data[latency_col],
                                    mode='markers',
                                    name=trace_name,
                                    marker=dict(color=size_color, size=3, opacity=0.7, symbol=symbol),
                                    showlegend=True
                                ))
                                
                            if 'moving_avg' in display_options and len(run_data) > moving_avg_window:
                                ma_col = f'{latency_type}_ma'
                                run_data[ma_col] = run_data[latency_col].rolling(window=moving_avg_window, min_periods=1).mean()
                                        
                                fig.add_trace(go.Scatter(
                                    x=run_data.index,
                                    y=run_data[ma_col],
                                    mode='lines',
                                    name=f'{trace_name} MA',
                                    line=dict(color=size_color, width=2, dash='solid'),
                                    opacity=0.9,
                                    showlegend=True
                                ))
                    
                    # Add statistical overlays per message size for better granularity
                    for size_idx, msg_size in enumerate(mechanism_message_sizes):
                        size_df = mechanism_df[mechanism_df['message_size'] == msg_size]
                        if len(size_df) == 0:
                            continue
                            
                        run_data_overlay = apply_sampling_strategy(size_df, sampling_strategy, MAX_POINTS_PER_RUN // len(mechanism_message_sizes))
                        run_data_overlay = run_data_overlay.reset_index(drop=True)
                        
                        # Use the same color as the corresponding trace
                        size_color = size_colors[size_idx % len(size_colors)]
                        
                        for latency_type in latency_types:
                            latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                            if latency_col in run_data_overlay.columns and not run_data_overlay[latency_col].isna().all():
                                # Add statistical overlays with message size context
                                add_statistical_overlays(fig, run_data_overlay, f"{mechanism} {msg_size}B", size_color, latency_type)
                    
                    anomaly_info = ""
                    if anomalies:
                        anomaly_info = f" ({len(anomalies)} anomalies)"
                
                # Add performance zones if enabled - show overall mechanism zones for reference
                if 'perf_zones' in view_options and len(mechanism_df) > 0:
                    # Add overall mechanism performance zones for each selected latency type
                    for latency_type in latency_types:
                        latency_col = 'one_way_latency_us' if latency_type == 'one_way' else 'round_trip_latency_us'
                        type_label = 'One-way' if latency_type == 'one_way' else 'Round-trip'
                        
                        if latency_col in mechanism_df.columns and not mechanism_df[latency_col].isna().all():
                            overall_p50 = mechanism_df[latency_col].quantile(0.5)
                            overall_p95 = mechanism_df[latency_col].quantile(0.95)
                            
                            # Use distinct colors for performance zones of different latency types
                            zone_color_p50 = '#10b981' if latency_type == 'one_way' else '#059669'  # Green family
                            zone_color_p95 = '#f59e0b' if latency_type == 'one_way' else '#d97706'  # Orange family
                            
                            fig.add_hline(y=overall_p50, line_dash="dash", line_color=zone_color_p50, 
                                         annotation_text=f"Overall P50 ({type_label})", annotation_position="bottom right")
                            fig.add_hline(y=overall_p95, line_dash="dash", line_color=zone_color_p95, 
                                         annotation_text=f"Overall P95 ({type_label})", annotation_position="bottom right")
                
                # Update chart layout for this mechanism (moved outside performance zones block)
                fig.update_layout(
                    title=f"{mechanism}{anomaly_info}",
                    xaxis_title="Sample Index",
                    yaxis_title="Latency (μs)",
                    yaxis_type=y_axis_scale,
                    yaxis_autorange=True if y_axis_range == 'auto' else False,
                    showlegend=True,
                    font=dict(size=12),
                    height=550,
                    template='plotly_white',
                    legend=dict(
                        orientation="h",
                        yanchor="top",
                        y=-0.15,
                        xanchor="center",
                        x=0.5,
                        bgcolor="rgba(255,255,255,0.9)",
                        bordercolor="rgba(0,0,0,0.2)",
                        borderwidth=1
                    ),
                    margin=dict(b=100)
                )
                
                charts.append(html.Div([
                    dcc.Graph(figure=fig, id=f'separate-chart-{mechanism}')
                ], className="chart-container"))
        
        # Combine all elements
        return html.Div([
            performance_note,
            *charts,
            stats_panel
        ])
    

    def render_throughput_tab(self, mechanisms: List, message_sizes: List,
                             moving_avg_window: int, display_options: List):
        """Render throughput trends tab."""
        # For now, use summary data if available
        summary_df = self.filter_data(self.data_store['summary_data'], mechanisms, message_sizes)
        
        if summary_df.empty:
            return html.Div("No throughput data available for selected filters.")
        
        # Create throughput comparison charts
        throughput_data = []
        for _, row in summary_df.iterrows():
            if not pd.isna(row['one_way_msgs_per_sec']):
                throughput_data.append({
                    'mechanism': row['mechanism'],
                    'message_size': row['message_size'],
                    'type': 'One-way',
                    'msgs_per_sec': row['one_way_msgs_per_sec'],
                    'mbps': row['one_way_bytes_per_sec'] / 1_000_000 if not pd.isna(row['one_way_bytes_per_sec']) else 0
                })
            if not pd.isna(row['round_trip_msgs_per_sec']):
                throughput_data.append({
                    'mechanism': row['mechanism'],
                    'message_size': row['message_size'],
                    'type': 'Round-trip',
                    'msgs_per_sec': row['round_trip_msgs_per_sec'],
                    'mbps': row['round_trip_bytes_per_sec'] / 1_000_000 if not pd.isna(row['round_trip_bytes_per_sec']) else 0
                })
        
        if not throughput_data:
            return html.Div("No throughput data available.")
        
        throughput_df = pd.DataFrame(throughput_data)
        
        # Messages per second chart
        msgs_fig = px.scatter(
            throughput_df,
            x='message_size',
            y='msgs_per_sec',
            color='mechanism',
            symbol='type',
            title="Throughput: Messages per Second vs Message Size",
            labels={'msgs_per_sec': 'Messages per Second', 'message_size': 'Message Size (bytes)'},
            hover_data=['mechanism']
        )
        
        # Megabits per second chart
        mbps_fig = px.scatter(
            throughput_df,
            x='message_size',
            y='mbps',
            color='mechanism',
            symbol='type',
            title="Throughput: Megabits per Second vs Message Size",
            labels={'mbps': 'Megabits per Second', 'message_size': 'Message Size (bytes)'},
            hover_data=['mechanism']
        )
        
        # Add Aggregated Statistics Table
        agg_table_div = html.Div()
        streaming_df = self.filter_data(self.data_store['streaming_data'], mechanisms, message_sizes)
        
        if not streaming_df.empty:
            # Create aggregated statistics by mechanism and message size
            agg_stats = streaming_df.groupby(['mechanism', 'message_size']).agg({
                'one_way_latency_us': ['count', 'mean', 'median', 'std', 'min', 'max'],
                'round_trip_latency_us': ['count', 'mean', 'median', 'std', 'min', 'max']
            }).round(2)
            
            # Flatten column names
            agg_stats.columns = [f"{col[1]}_{col[0]}" for col in agg_stats.columns]
            agg_stats = agg_stats.reset_index()
            
            # Rename columns for better display
            agg_column_mapping = {
                'mechanism': 'Mechanism',
                'message_size': 'Message Size (bytes)',
                'count_one_way_latency_us': 'One-way Count',
                'mean_one_way_latency_us': 'One-way Mean (μs)',
                'median_one_way_latency_us': 'One-way Median (μs)',
                'std_one_way_latency_us': 'One-way Std Dev (μs)',
                'min_one_way_latency_us': 'One-way Min (μs)',
                'max_one_way_latency_us': 'One-way Max (μs)',
                'count_round_trip_latency_us': 'Round-trip Count',
                'mean_round_trip_latency_us': 'Round-trip Mean (μs)',
                'median_round_trip_latency_us': 'Round-trip Median (μs)',
                'std_round_trip_latency_us': 'Round-trip Std Dev (μs)',
                'min_round_trip_latency_us': 'Round-trip Min (μs)',
                'max_round_trip_latency_us': 'Round-trip Max (μs)'
            }
            agg_stats = agg_stats.rename(columns=agg_column_mapping)
            
            agg_table = dash_table.DataTable(
                id='aggregated-table',
                data=agg_stats.to_dict('records'),
                columns=[{"name": i, "id": i} for i in agg_stats.columns],
                style_table={'overflowX': 'auto'},
                style_cell={
                    'textAlign': 'left',
                    'padding': '10px',
                    'fontFamily': 'Arial, sans-serif',
                    'fontSize': '14px'
                },
                style_header={
                    'backgroundColor': '#e8f5e8',
                    'fontWeight': 'bold',
                    'color': '#2e7d32'
                },
                style_data_conditional=[
                    {
                        'if': {'row_index': 'odd'},
                        'backgroundColor': '#f8f9fa'
                    }
                ],
                sort_action="native",
                filter_action="native",
                page_action="native",
                page_current=0,
                page_size=10,
            )
            
            agg_table_div = html.Div([
                html.H3("Performance Statistics by Configuration"),
                html.P("Statistical summary grouped by mechanism and message size including count, mean, median, standard deviation, min, and max values. "
                       "Use this data to understand throughput capabilities and latency distributions.",
                       style={'color': '#6b7280', 'margin-bottom': '20px', 'fontSize': '0.95rem'}),
                agg_table
            ], className="dash-table-container")

        return html.Div([
            html.Div([
                dcc.Graph(figure=msgs_fig)
            ], className="chart-container"),
            html.Div([
                dcc.Graph(figure=mbps_fig)
            ], className="chart-container"),
            agg_table_div
        ])
    

    def run(self, host='127.0.0.1', port=8050, debug=True):
        """Run the dashboard server."""
        logger.info(f"Starting dashboard at http://{host}:{port}")
        self.app.run(host=host, port=port, debug=debug)

def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description="Rusty-Comms Benchmark Results Dashboard")
    parser.add_argument("--dir", required=True, help="Directory containing benchmark result files")
    parser.add_argument("--host", default="127.0.0.1", help="Host to bind to (default: 127.0.0.1)")
    parser.add_argument("--port", type=int, default=8050, help="Port to bind to (default: 8050)")
    parser.add_argument("--debug", action="store_true", default=False, help="Enable debug mode (can cause reloading issues)")
    
    args = parser.parse_args()
    
    # Validate directory
    results_dir = Path(args.dir)
    if not results_dir.exists():
        logger.error(f"Results directory does not exist: {results_dir}")
        return 1
    
    # Process data
    processor = BenchmarkDataProcessor(results_dir)
    data_store = processor.process_all_data()
    
    if data_store['summary_data'].empty and data_store['streaming_data'].empty:
        logger.error("No valid benchmark data found in the specified directory")
        return 1
    
    # Setup and run dashboard
    dashboard = DashboardApp(data_store, initial_dir=args.dir)
    dashboard.run(host=args.host, port=args.port, debug=args.debug)
    
    return 0

# Modern CSS styling
app_css = """
/* Import modern fonts */
@import url('https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700&display=swap');

/* Global styles */
* {
    font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
}

body {
    margin: 0;
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    min-height: 100vh;
}

.header-title {
    text-align: center;
    color: #ffffff;
    margin: 0 0 30px 0;
    font-size: 2.5rem;
    font-weight: 700;
    text-shadow: 0 2px 4px rgba(0,0,0,0.1);
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    padding: 30px 0;
    margin: -20px -20px 30px -20px;
    border-radius: 0 0 20px 20px;
    box-shadow: 0 4px 20px rgba(0,0,0,0.1);
}

.dashboard-container {
    display: flex;
    height: 100vh;
    background: #f8fafc;
}

.sidebar {
    width: 240px;
    padding: 16px;
    background: rgba(255, 255, 255, 0.95);
    backdrop-filter: blur(10px);
    border-right: none;
    box-shadow: 4px 0 20px rgba(0,0,0,0.08);
    overflow-y: auto;
    border-radius: 0 20px 20px 0;
    margin: 20px 0 20px 20px;
}

.main-content {
    flex: 1;
    padding: 25px;
    overflow-y: auto;
    background: #f8fafc;
}

/* Filter sections */
.filter-section {
    background: white;
    border-radius: 16px;
    padding: 20px;
    margin-bottom: 20px;
    box-shadow: 0 4px 15px rgba(0,0,0,0.08);
    border: 1px solid rgba(255,255,255,0.2);
}

.filter-checklist {
    margin-bottom: 15px;
}

.filter-checklist label {
    font-weight: 500;
    color: #374151;
    margin-bottom: 12px;
    display: block;
    font-size: 0.9rem;
}

/* Slider styling */
.slider {
    margin-bottom: 20px;
}

.slider .rc-slider {
    border-radius: 8px;
}

.slider .rc-slider-track {
    background: linear-gradient(90deg, #667eea 0%, #764ba2 100%);
    border-radius: 8px;
    height: 6px;
}

.slider .rc-slider-handle {
    background: white;
    border: 3px solid #667eea;
    width: 20px;
    height: 20px;
    margin-top: -7px;
    box-shadow: 0 2px 8px rgba(0,0,0,0.15);
}

/* Tab styling */
.tab-content {
    background: white;
    border-radius: 20px;
    padding: 30px;
    box-shadow: 0 8px 32px rgba(0,0,0,0.12);
    margin-bottom: 20px;
    border: 1px solid rgba(255,255,255,0.2);
}

/* Chart containers */
.chart-container {
    background: white;
    border-radius: 16px;
    padding: 25px;
    margin-bottom: 25px;
    box-shadow: 0 4px 20px rgba(0,0,0,0.08);
    border: 1px solid rgba(255,255,255,0.2);
    transition: transform 0.2s ease, box-shadow 0.2s ease;
}

.chart-container:hover {
    transform: translateY(-2px);
    box-shadow: 0 8px 30px rgba(0,0,0,0.12);
}

/* Table styling */
.dash-table-container {
    background: white;
    border-radius: 16px;
    padding: 20px;
    margin: 25px 0;
    box-shadow: 0 4px 20px rgba(0,0,0,0.08);
    border: 1px solid rgba(255,255,255,0.2);
}

.dash-table-container .dash-spreadsheet-container {
    border-radius: 12px;
    overflow: hidden;
}

.dash-table-container .dash-spreadsheet-container .dash-spreadsheet-inner table {
    border-radius: 12px;
}

/* Performance note styling */
.performance-note {
    background: linear-gradient(135deg, #fef7cd 0%, #fef3c7 100%);
    border: 1px solid #f59e0b;
    border-radius: 12px;
    padding: 16px 20px;
    margin-bottom: 25px;
    box-shadow: 0 2px 8px rgba(245, 158, 11, 0.1);
}

.performance-note p {
    margin: 0;
    color: black;
    font-weight: 500;
}

/* Section headers */
h3 {
    color: #1f2937;
    font-weight: 600;
    font-size: 1.5rem;
    margin-bottom: 16px;
    display: flex;
    align-items: center;
    gap: 8px;
}

/* Labels */
label {
    color: #374151;
    font-weight: 500;
    font-size: 0.9rem;
    margin-bottom: 8px;
    display: block;
}

/* Checkboxes and inputs */
input[type="checkbox"] {
    accent-color: #667eea;
    transform: scale(1.1);
    margin-right: 8px;
}

/* Scrollbar styling */
::-webkit-scrollbar {
    width: 8px;
}

::-webkit-scrollbar-track {
    background: #f1f5f9;
    border-radius: 4px;
}

::-webkit-scrollbar-thumb {
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    border-radius: 4px;
}

::-webkit-scrollbar-thumb:hover {
    background: linear-gradient(135deg, #5a6fd8 0%, #6b4190 100%);
}

/* Modern card effect for graphs */
._dash-graph {
    border-radius: 12px;
    overflow: hidden;
}

/* Dark mode toggle styling */
.dark-mode-toggle input[type="checkbox"] {
    appearance: none;
    width: 50px;
    height: 25px;
    border-radius: 25px;
    background: #ccc;
    position: relative;
    outline: none;
    cursor: pointer;
    transition: background 0.3s;
}

.dark-mode-toggle input[type="checkbox"]:checked {
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
}

.dark-mode-toggle input[type="checkbox"]:before {
    content: '';
    position: absolute;
    top: 2px;
    left: 2px;
    width: 21px;
    height: 21px;
    border-radius: 50%;
    background: white;
    transition: transform 0.3s;
    box-shadow: 0 2px 4px rgba(0,0,0,0.2);
}

.dark-mode-toggle input[type="checkbox"]:checked:before {
    transform: translateX(25px);
}


/* Responsive adjustments */
@media (max-width: 1200px) {
    .sidebar {
        width: 280px;
    }
}

@media (max-width: 768px) {
    .dashboard-container {
        flex-direction: column;
    }
    
    .sidebar {
        width: 100%;
        border-radius: 20px 20px 0 0;
        margin: 20px 20px 0 20px;
    }
    
    .main-content {
        padding: 20px;
    }
}

/* New Time Series Tab Styles */
.preset-btn {
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    color: white;
    border: none;
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 0.9rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.3s ease;
    min-width: 140px;
    box-shadow: 0 2px 8px rgba(102, 126, 234, 0.3);
}

.preset-btn:hover {
    transform: translateY(-2px);
    box-shadow: 0 4px 12px rgba(102, 126, 234, 0.4);
}

.preset-btn-secondary {
    background: #6b7280;
    color: white;
    border: none;
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 0.9rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.3s ease;
    min-width: 140px;
    box-shadow: 0 2px 8px rgba(107, 114, 128, 0.3);
}

.preset-btn-secondary:hover {
    background: #4b5563;
    transform: translateY(-2px);
    box-shadow: 0 4px 12px rgba(107, 114, 128, 0.4);
}

.chart-layout-radio .form-check-input {
    margin-right: 8px;
}

.chart-layout-radio .form-check-label {
    font-size: 0.9rem;
    color: #374151;
    font-weight: 500;
}

.axis-scale-radio .form-check-input,
.axis-range-radio .form-check-input {
    margin-right: 6px;
}

.axis-scale-radio .form-check-label,
.axis-range-radio .form-check-label {
    font-size: 0.85rem;
    color: #6b7280;
}

.sampling-dropdown .Select-control {
    border-radius: 8px;
    border: 1px solid #d1d5db;
    font-size: 0.9rem;
}

.sampling-dropdown .Select-menu-outer {
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0,0,0,0.1);
}

/* Performance note styling */
.performance-note {
    background: #fef3c7;
    border: 1px solid #f59e0b;
    border-radius: 8px;
    padding: 12px 16px;
    margin-bottom: 20px;
    font-size: 0.9rem;
    color: black;
}

/* Enhanced chart container */
.chart-container {
    background: white;
    border-radius: 12px;
    padding: 15px;
    margin-bottom: 20px;
    box-shadow: 0 2px 10px rgba(0,0,0,0.05);
    border: 1px solid #e5e7eb;
}

/* Statistical overlays section styling */
.statistical-controls {
    background: #f1f5f9;
    border: 1px solid #cbd5e1;
    border-radius: 8px;
    padding: 15px;
    margin-bottom: 20px;
}

.preset-controls {
    background: #fef7cd;
    border: 1px solid #f59e0b;
    border-radius: 8px;
    padding: 15px;
    margin-bottom: 20px;
}

/* Real-time stats table enhancements */
.stats-panel {
    background: #f8fafc;
    border: 1px solid #e2e8f0;
    border-radius: 8px;
    padding: 20px;
    margin-bottom: 30px;
}

.stats-panel h4 {
    color: #1f2937;
    margin-bottom: 15px;
    font-weight: 600;
}

/* Anomaly alert styling */
.anomaly-alert {
    background: #fef2f2;
    border: 1px solid #fecaca;
    border-radius: 8px;
    padding: 15px;
    margin-bottom: 20px;
}

.anomaly-alert h5 {
    color: #dc2626;
    margin-bottom: 10px;
    font-weight: 600;
}

.anomaly-alert ul {
    margin-bottom: 0;
    padding-left: 20px;
}

.anomaly-alert li {
    color: #374151;
    margin-bottom: 5px;
}
"""

if __name__ == "__main__":
    exit(main())
