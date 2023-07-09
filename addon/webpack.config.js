const HtmlWebpackPlugin = require('html-webpack-plugin');
const path = require('path');

module.exports = {
	mode: 'production',
	entry: {
		background: './background',
		content: './content',
		injected: './injected',
		popup: './popup',
	},
	output: {
		filename: '[name].js',
		path: __dirname + '/../addon-dist',
	},
	module: {
		rules: [
			{
				test: /\.tsx?$/,
				use: 'ts-loader',
				exclude: /node_modules/,
			},
			{
				test: /\.css$/,
				use: [
					'style-loader',
					'css-loader'
				],
			},
		]
	},
	plugins: [
		new HtmlWebpackPlugin({
			filename: 'popup.html',
			chunks: ['popup'],
		})
	],
	resolve: {
		extensions: ['.tsx', '.ts', '.js'],
	},
};
