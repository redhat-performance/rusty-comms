# Rusty Comms Performance Dashboard

A comprehensive interactive web dashboard for analyzing and comparing Inter-Process Communication (IPC) mechanism performance in Rust. Built with Plotly Dash, this tool provides detailed visualizations and statistical analysis of latency, throughput, and reliability metrics across different communication mechanisms.

## 🚀 Features

### 📊 **Summary Tab**
Comprehensive performance overview with statistical analysis and insights:

- **Performance Overview Cards**: Single-row display of key metrics
  - Best Overall Mechanism (lowest P50 latency)
  - Performance Range (min-max latency with average)
  - Peak Throughput (highest messages/sec with mechanism)
  - Best Max Latency (lowest worst-case latency)
  - Key Insights (AI-generated performance recommendations)

- **Head-to-Head Comparison Matrices**: Side-by-side comparison tables
  - P50 Latency Comparison Matrix (typical performance)
  - Max Latency Comparison Matrix (worst-case performance)
  - Relative performance percentages compared to best mechanism

- **Performance Insights**: Intelligent analysis with actionable recommendations
  - Performance recommendations based on data analysis
  - Statistical comparisons between mechanisms
  - Best practices and optimization suggestions

- **Latency Analysis**: Detailed latency performance breakdown
  - Average and Maximum Latency Bar Charts (by mechanism and message size)
  - Four pivot tables showing latency data:
    - Max Latency: One-way vs Round-trip (side-by-side)
    - Average Latency: One-way vs Round-trip (side-by-side)
  - Message size as first column, mechanisms as subsequent columns

- **Throughput Analysis**: Performance and efficiency metrics
  - Messages/sec and Bytes/sec Bar Charts (by mechanism and message size)
  - Four pivot tables showing throughput data:
    - Message Throughput: One-way vs Round-trip messages/sec (side-by-side)
    - Data Throughput: One-way vs Round-trip MB/sec (side-by-side)
  - Message size as first column, mechanisms as subsequent columns

### 📈 **Time Series Tab**
Interactive time-series analysis with advanced visualization controls:

- **Real-time Data Visualization**: Interactive scatter plots with streaming data
- **Moving Averages**: Configurable rolling averages (1-100 point window)
- **Outlier Detection**: Dynamic threshold-based outlier highlighting
- **Custom Display Options**: 
  - Toggle moving average lines (solid lines)
  - Toggle outlier highlighting
  - Adjustable dot size (1-10 pixels)
  - Adjustable line width (1-5 pixels)
- **Data Downsampling**: Automatic sampling for large datasets (>10K points)
- **Interactive Filtering**: Real-time chart updates based on filter selections

### 🎛️ **Global Controls**

- **Data Import**: Directory selection dropdown with custom path input
- **Mechanism Filtering**: Multi-select checkboxes for IPC mechanisms
  - SharedMemory
  - TcpSocket  
  - UnixDomainSocket
- **Message Size Filtering**: Multi-select checkboxes for message sizes
  - 64, 128, 256, 512, 1024, 4096, 8192 bytes
- **Real-time Data Reload**: Refresh data without restarting dashboard

## 🛠️ Installation & Setup

### Prerequisites
- Python 3.13+
- Virtual environment support

### Installation
```bash
# Navigate to project directory
cd rusty-comms

# Create and activate virtual environment
python -m venv venv
source venv/bin/activate  # On Windows: venv\Scripts\activate

# Install dashboard dependencies
pip install -r requirements.txt
```

### Required Dependencies
```
dash==3.2.0
plotly==5.24.1
pandas==2.2.3
numpy==2.1.1
dash-table==5.0.0
```

## 🚀 Usage

### Starting the Dashboard
```bash
# Basic usage (current directory)
python dashboard.py --dir .

# Specify custom data directory
python dashboard.py --dir /path/to/benchmark/data

# Custom host and port
python dashboard.py --dir . --host 0.0.0.0 --port 8080

# Enable debug mode (warning: may cause auto-reloading)
python dashboard.py --dir . --debug
```

### Command Line Options
- `--dir`: Directory containing benchmark data files (default: current directory)
- `--host`: Host address to bind to (default: 127.0.0.1)
- `--port`: Port number to serve on (default: 8050)
- `--debug`: Enable debug mode with auto-reloading (default: False)

### Accessing the Dashboard
Open your web browser and navigate to:
```
http://127.0.0.1:8050
```

## 📁 Data Format

The dashboard automatically discovers and loads data files in the specified directory:

### Supported File Types

#### **Summary Data** (`*_results.json`)
```json
{
  "mechanism": "SharedMemory",
  "message_size": 1024,
  "one_way_p50_latency_us": 1.23,
  "one_way_p95_latency_us": 2.45,
  "one_way_p99_latency_us": 3.67,
  "round_trip_p50_latency_us": 2.46,
  "round_trip_p95_latency_us": 4.90,
  "round_trip_p99_latency_us": 7.34,
  "one_way_msgs_per_sec": 812347.5,
  "one_way_bytes_per_sec": 831587840.0,
  "round_trip_msgs_per_sec": 406173.75,
  "round_trip_bytes_per_sec": 415794176.0
}
```

#### **Streaming Data** (`*_streaming.json` or `*.csv`)
**JSON Format:**
```json
{
  "mechanism": "TcpSocket",
  "message_size": 512,
  "one_way_latency_us": 1.45,
  "round_trip_latency_us": 2.89,
  "timestamp_us": 1692123456789
}
```

**CSV Format:**
```csv
mechanism,message_size,one_way_latency_us,round_trip_latency_us,timestamp_us
UnixDomainSocket,256,0.89,1.78,1692123456789
```

### File Discovery
- Recursive directory scanning with `.gitignore` pattern exclusion
- Automatic file type detection based on naming patterns
- Support for mixed file formats in the same directory
- Real-time file validation and error reporting

## 🎨 Interface Guide

### Navigation
- **Tab Selection**: Click tabs to switch between Summary and Time Series views
- **Filter Panel**: Left sidebar with mechanism and message size selections
- **Data Import**: Top section with directory selection and reload controls

### Interactive Features

#### **Summary Tab**
- **Performance Cards**: Hover for detailed tooltips
- **Comparison Tables**: Click column headers to sort
- **Charts**: Hover for precise values, click legend to toggle series

#### **Time Series Tab**
- **Chart Interaction**: Zoom, pan, and hover for detailed point information
- **Moving Average Slider**: Adjust window size (1-100 points)
- **Display Toggles**: Show/hide moving averages and outliers
- **Threshold Controls**: Set custom outlier detection thresholds per mechanism
- **Styling Controls**: Adjust dot size and line width

### Data Precision
- **Latency**: Displayed in microseconds (μs) with 2 decimal places
- **Throughput**: Messages/sec with 3 decimal places, MB/sec with 1 decimal place
- **Comparison**: Relative percentages with 1 decimal place

## 🔧 Technical Architecture

### Backend Processing
- **Data Loader**: Multi-format file discovery and validation
- **Statistical Engine**: Percentile calculations from streaming data
- **Performance Analytics**: Comparative analysis and insights generation
- **Memory Management**: Efficient handling of large datasets (5M+ records)

### Frontend Framework
- **Plotly Dash**: Reactive web application framework
- **Interactive Components**: Real-time chart updates and filtering
- **Responsive Design**: Adaptive layout for different screen sizes
- **Modern UI**: Clean, professional styling with intuitive navigation

### Performance Optimizations
- **Data Downsampling**: Automatic reduction for large time series (>10K points)
- **Lazy Loading**: On-demand data processing and chart generation
- **Caching**: Efficient reuse of calculated statistics
- **Incremental Updates**: Minimal re-rendering on filter changes

## 🐛 Troubleshooting

### Common Issues

#### **Dashboard Won't Start**
```bash
# Check Python version
python --version  # Should be 3.13+

# Verify dependencies
pip list | grep dash

# Check for port conflicts
lsof -i :8050
```

#### **Data Not Loading**
- Verify file formats match expected structure
- Check directory permissions
- Ensure files contain valid JSON/CSV data
- Review console logs for specific error messages

#### **Performance Issues**
- Large datasets (>1M records) may require additional memory
- Enable data downsampling for smoother interactions
- Consider filtering to smaller subsets for initial analysis

#### **Auto-Reloading Issues**
- Disable debug mode: `--debug false`
- Restart dashboard if experiencing continuous reloads
- Check file system watchers aren't triggering false changes

### Debug Mode
```bash
# Enable verbose logging
python dashboard.py --dir . --debug

# Check browser console for JavaScript errors
# Monitor terminal output for Python exceptions
```

## 📊 Example Analysis Workflow

1. **Start Dashboard**: `python dashboard.py --dir ./benchmark_results`
2. **Load Data**: Use directory dropdown to select benchmark data folder
3. **Filter Mechanisms**: Select specific IPC mechanisms to compare
4. **Review Summary**: Analyze performance overview cards and insights
5. **Compare Mechanisms**: Review head-to-head comparison matrices
6. **Examine Details**: Study latency and throughput tables
7. **Time Series Analysis**: Switch to Time Series tab for temporal patterns
8. **Adjust Parameters**: Modify moving averages and outlier thresholds
9. **Export Insights**: Use browser tools to capture charts and tables

## 🤝 Contributing

The dashboard is part of the larger Rusty Comms benchmarking suite. Contributions welcome for:

- Additional visualization types
- Enhanced statistical analysis
- Performance optimizations
- UI/UX improvements
- Additional file format support

## 📝 License

This dashboard is part of the Rusty Comms project. See the main project LICENSE file for details.

## 🔗 Related

- **Main Project**: [Rusty Comms IPC Benchmarking Suite](../README.md)
- **Benchmark Runner**: See main documentation for data generation
- **Contributing Guide**: [CONTRIBUTING.md](../CONTRIBUTING.md)